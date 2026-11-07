param(
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Folder,
    [ValidateRange(10, 600)] [int] $Seconds = 60
)

# Opens a large folder, runs Uia-Client.ps1 against the window and samples the private working
# set of the app's process tree and whether Windows considers the window hung.
$ErrorActionPreference = 'Stop'
$appPath = (Resolve-Path -LiteralPath $App).Path
$folderPath = (Resolve-Path -LiteralPath $Folder).Path
$tauri = (Split-Path -Leaf $appPath) -like '*tauri*'
$run = New-Item -ItemType Directory -Path (Join-Path $PSScriptRoot ('../target/uia-' + [guid]::NewGuid().ToString('N')))
$clientLog = Join-Path $run.FullName 'client.jsonl'

. (Join-Path $PSScriptRoot 'PrivateWorkingSet.ps1')
if (-not ('FirewolWindows' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class FirewolWindows {
    delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern int GetWindowTextLengthW(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr hwnd);
    public static IntPtr Find(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, _) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner == pid && IsWindowVisible(hwnd) && GetWindowTextLengthW(hwnd) > 0) { found = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
}

$saved = @{}
$variables = [ordered]@{
    LOCALAPPDATA = (New-Item -ItemType Directory -Path (Join-Path $run.FullName 'local')).FullName
    WEBVIEW2_USER_DATA_FOLDER = Join-Path $run.FullName 'webview'
    FIREWOL_OPEN = $folderPath
}
$appProcess = $null
$client = $null
try {
    foreach ($name in $variables.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $variables[$name])
    }
    $arguments = if ($tauri) { @() } else { @('--open', "`"$folderPath`"") }
    $appProcess = Start-Process -FilePath $appPath -ArgumentList $arguments -PassThru
    $window = [IntPtr]::Zero
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    while ($window -eq [IntPtr]::Zero -and $timer.Elapsed.TotalSeconds -lt 30) {
        Start-Sleep -Milliseconds 100
        $window = [FirewolWindows]::Find([uint32]$appProcess.Id)
    }
    if ($window -eq [IntPtr]::Zero) { throw 'The Firewol window did not appear' }
    Start-Sleep -Seconds 3
    $before = [FirewolM0Memory]::Measure([uint32[]]@($appProcess.Id))
    $client = Start-Process -FilePath powershell.exe -WindowStyle Hidden -PassThru -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$(Join-Path $PSScriptRoot 'Uia-Client.ps1')`"",
        '-ProcessId', $appProcess.Id, '-Seconds', $Seconds, '-Output', "`"$clientLog`"")
    $peak = $before
    $peakProcesses = @()
    $lastBreakdown = -1000
    $samples = 0
    $hungSamples = 0
    $hungSince = $null
    $longestHungMs = 0
    $timer.Restart()
    while ($timer.Elapsed.TotalSeconds -lt $Seconds + 5) {
        if ($appProcess.HasExited) { throw 'Firewol exited during the stress test' }
        $bytes = [FirewolM0Memory]::Measure([uint32[]]@($appProcess.Id))
        if ($bytes -gt $peak -and $timer.ElapsedMilliseconds - $lastBreakdown -ge 1000) {
            # Which process holds the memory at a new peak (throttled; the CIM query is slow).
            $lastBreakdown = $timer.ElapsedMilliseconds
            $all = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe' OR ProcessId=$($appProcess.Id)")
            $peakProcesses = @($all | Where-Object { $_.ProcessId -eq $appProcess.Id -or $_.CommandLine -match [regex]::Escape($run.FullName) } | ForEach-Object {
                [ordered]@{
                    type = if ($_.CommandLine -match '--type=([\w-]+)') { $Matches[1] } else { $_.Name }
                    bytes = [FirewolM0Memory]::Measure([uint32[]]@($_.ProcessId)) - (($all | Where-Object ParentProcessId -eq $_.ProcessId | ForEach-Object { [FirewolM0Memory]::Measure([uint32[]]@($_.ProcessId)) } | Measure-Object -Sum).Sum)
                }
            })
        }
        $peak = [Math]::Max([uint64]$peak, $bytes)
        $samples++
        if ([FirewolWindows]::IsHungAppWindow($window)) {
            $hungSamples++
            if ($null -eq $hungSince) { $hungSince = $timer.ElapsedMilliseconds }
            $longestHungMs = [Math]::Max($longestHungMs, $timer.ElapsedMilliseconds - $hungSince)
        } else {
            $hungSince = $null
        }
        Start-Sleep -Milliseconds 200
    }
    $final = [FirewolM0Memory]::Measure([uint32[]]@($appProcess.Id))
    $lines = @(if (Test-Path -LiteralPath $clientLog) { Get-Content -LiteralPath $clientLog | ForEach-Object { $_ | ConvertFrom-Json } })
    $finds = @($lines | Where-Object { $_.elements -ge 0 } | ForEach-Object { [int64]$_.find_ms } | Sort-Object)
    $result = [ordered]@{
        executable = Split-Path -Leaf $appPath
        folder = $folderPath
        seconds = $Seconds
        bytes_before_client = [uint64]$before
        peak_bytes = [uint64]$peak
        processes_near_peak = $peakProcesses
        final_bytes = [uint64]$final
        samples = $samples
        hung_samples = $hungSamples
        longest_hung_ms = $longestHungMs
        client_iterations = $lines.Count
        client_completed_walks = $finds.Count
        max_elements = ($lines | ForEach-Object { [int]$_.elements } | Measure-Object -Maximum).Maximum
        median_find_ms = if ($finds.Count) { $finds[[int][Math]::Floor($finds.Count / 2)] } else { $null }
        max_find_ms = if ($finds.Count) { $finds[-1] } else { $null }
        scroll_steps = @($lines | Where-Object { $_.scroll -match '^\d+$' }).Count
        events = if ($lines.Count) { $lines[-1].events } else { 0 }
        errors = @($lines | Where-Object { $_.error } | ForEach-Object { $_.error } | Select-Object -Unique)
    }
    $result | ConvertTo-Json -Depth 4 | Tee-Object -FilePath (Join-Path $run.FullName 'result.json')
} finally {
    if ($client -and -not $client.HasExited) { $client.Kill() }
    if ($appProcess -and -not $appProcess.HasExited) {
        $appProcess.Kill($true)
        $null = $appProcess.WaitForExit(5000)
    }
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
}
