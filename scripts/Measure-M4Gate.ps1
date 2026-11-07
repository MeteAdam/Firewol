param(
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Fixture,
    [ValidateRange(1, 4)] [int] $Windows = 4,
    [ValidateRange(5, 120)] [int] $IdleSeconds = 10
)

# M4.1 gate: one window (startup, scroll, folder index, search, idle), then four windows with
# split panes, a concurrent one-million-record index build and a 256 MiB copy.
$ErrorActionPreference = 'Stop'
$appPath = (Resolve-Path -LiteralPath $App).Path
$fixturePath = (Resolve-Path -LiteralPath $Fixture).Path
$cli = Join-Path (Split-Path -Parent $appPath) 'firewol-m0.exe'
$info = Get-Content -LiteralPath (Join-Path $fixturePath 'fixture.json') -Raw | ConvertFrom-Json
$run = New-Item -ItemType Directory -Path (Join-Path $PSScriptRoot ('../target/measure-m4-' + [guid]::NewGuid().ToString('N')))
$reports = New-Item -ItemType Directory -Path (Join-Path $run.FullName 'reports')
$work = New-Item -ItemType Directory -Path (Join-Path $run.FullName 'work')

. (Join-Path $PSScriptRoot 'PrivateWorkingSet.ps1')

$saved = @{}
$variables = [ordered]@{
    LOCALAPPDATA = (New-Item -ItemType Directory -Path (Join-Path $run.FullName 'local')).FullName
    WEBVIEW2_USER_DATA_FOLDER = Join-Path $run.FullName 'webview'
    FIREWOL_OPEN = Join-Path $fixturePath 'files'
    FIREWOL_SPIKE_REPORT = $reports.FullName
    FIREWOL_SPIKE_WINDOWS = "$Windows"
    FIREWOL_SPIKE_IDLE_MS = "$($IdleSeconds * 1000)"
    FIREWOL_M0_ROOT = $work.FullName
}
$phases = [ordered]@{}
$appProcess = $null
$helper = $null

function Get-Breakdown {
    $all = @(Get-CimInstance Win32_Process)
    $pending = [System.Collections.Generic.Queue[uint32]]::new()
    $pending.Enqueue([uint32]$appProcess.Id)
    if ($helper -and -not $helper.HasExited) { $pending.Enqueue([uint32]$helper.Id) }
    $rows = @()
    while ($pending.Count -gt 0) {
        $id = $pending.Dequeue()
        $process = $all | Where-Object ProcessId -eq $id
        if (-not $process) { continue }
        $kind = if ($process.CommandLine -match '--type=([\w-]+)') { $Matches[1] } else { 'main' }
        $rows += [ordered]@{ name = $process.Name; type = $kind; id = $id; bytes = [FirewolM0Memory]::Measure([uint32[]]@($id)) - (($all | Where-Object ParentProcessId -eq $id | ForEach-Object { [FirewolM0Memory]::Measure([uint32[]]@($_.ProcessId)) } | Measure-Object -Sum).Sum) }
        $all | Where-Object ParentProcessId -eq $id | ForEach-Object { $pending.Enqueue([uint32]$_.ProcessId) }
    }
    $rows
}

function Measure-Phase([string] $Name, [scriptblock] $Done, [int] $TimeoutSeconds) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $peak = 0
    $samples = 0
    do {
        if ($appProcess.HasExited) { throw "Firewol exited during $Name" }
        $ids = @($appProcess.Id)
        if ($helper -and -not $helper.HasExited) { $ids += $helper.Id }
        $bytes = [FirewolM0Memory]::Measure([uint32[]]$ids)
        $peak = [Math]::Max([uint64]$peak, $bytes)
        $samples++
        if (& $Done) { break }
        if ($timer.Elapsed.TotalSeconds -ge $TimeoutSeconds) { throw "Timed out during $Name" }
        Start-Sleep -Milliseconds 20
    } while ($true)
    $phases[$Name] = [ordered]@{
        peak_bytes = $peak
        samples = $samples
        elapsed_ms = $timer.ElapsedMilliseconds
        processes_at_end = @(Get-Breakdown)
    }
    Write-Information ('{0}: peak {1:N0} bytes' -f $Name, $peak) -InformationAction Continue
}

function Test-Report([string] $Label) {
    $path = Join-Path $reports.FullName "$Label.json"
    (Test-Path -LiteralPath $path) -and (Get-Item -LiteralPath $path).Length -gt 0
}

try {
    foreach ($name in $variables.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $variables[$name])
    }
    $appProcess = Start-Process -FilePath $appPath -PassThru
    Measure-Phase -Name 'startup_scroll_index_search' -Done { Test-Report 'main' } -TimeoutSeconds 300
    $main = Get-Content -LiteralPath (Join-Path $reports.FullName 'main.json') -Raw | ConvertFrom-Json
    if ($main.error) { throw "The main window benchmark failed: $($main.error)" }
    $idle = [System.Diagnostics.Stopwatch]::StartNew()
    Measure-Phase -Name 'idle_one_window' -Done { $idle.Elapsed.TotalSeconds -ge ($IdleSeconds - 2) } -TimeoutSeconds ($IdleSeconds + 10)

    $labels = @(1..($Windows - 1) | ForEach-Object { "window-$_" })
    if ($labels.Count -gt 0) {
        Measure-Phase -Name 'open_windows_split' -Done { @($labels | Where-Object { -not (Test-Report $_) }).Count -eq 0 } -TimeoutSeconds 240
        $settle = [System.Diagnostics.Stopwatch]::StartNew()
        Measure-Phase -Name 'idle_all_windows' -Done { $settle.Elapsed.TotalSeconds -ge 5 } -TimeoutSeconds 15
    }

    $helper = Start-Process -FilePath $cli -ArgumentList 'index-load', $info.index_rows -WindowStyle Hidden -PassThru
    Measure-Phase -Name 'index_one_million' -Done { $helper.HasExited } -TimeoutSeconds 300
    if ($helper.ExitCode -ne 0) { throw "Index workload exited with code $($helper.ExitCode)" }
    $helper = Start-Process -FilePath $cli -ArgumentList 'copy-load' -WindowStyle Hidden -PassThru
    Measure-Phase -Name 'copy_256_mib' -Done { $helper.HasExited } -TimeoutSeconds 60
    if ($helper.ExitCode -ne 0) { throw "Copy workload exited with code $($helper.ExitCode)" }

    $windowReports = [ordered]@{}
    foreach ($label in @('main') + $labels) {
        $windowReports[$label] = Get-Content -LiteralPath (Join-Path $reports.FullName "$label.json") -Raw | ConvertFrom-Json
    }
    $oneWindow = [Math]::Max($phases['startup_scroll_index_search'].peak_bytes, $phases['idle_one_window'].peak_bytes)
    $worst = ($phases.Values | ForEach-Object { $_.peak_bytes } | Measure-Object -Maximum).Maximum
    $gate = [ordered]@{
        one_window_peak_bytes = [uint64]$oneWindow
        one_window_idle_bytes = [uint64]$phases['idle_one_window'].peak_bytes
        one_window_idle_near_150mb = $phases['idle_one_window'].peak_bytes -le 157286400
        worst_case_peak_bytes = [uint64]$worst
        worst_case_below_250mb = $worst -lt 250000000
        first_list_below_300ms = $main.firstListMs -lt 300
        scroll_at_least_60fps = $main.scrollFps -ge 60
        search_p95_below_100ms = $main.searchP95Ms -lt 100
        last_row_verified = [bool]$main.lastRowVerified
    }
    $result = [ordered]@{
        executable = Split-Path -Leaf $appPath
        fixture = $info
        windows = $Windows
        reports = $windowReports
        phases = $phases
        gate = $gate
    }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $run.FullName 'result.json') -Encoding utf8
    $gate | ConvertTo-Json
    Write-Information "Results saved to $($run.FullName)" -InformationAction Continue
} finally {
    if ($helper -and -not $helper.HasExited) { $helper.Kill() }
    if ($appProcess -and -not $appProcess.HasExited) {
        $appProcess.Kill($true)
        $null = $appProcess.WaitForExit(5000)
    }
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
}
