param([string] $App = '.\target\release\firewol-tauri.exe')

$ErrorActionPreference = 'Stop'
$appPath = (Resolve-Path -LiteralPath $App).Path
$run = New-Item -ItemType Directory -Path (Join-Path $PSScriptRoot ('../target/interface-' + [guid]::NewGuid().ToString('N')))
$source = New-Item -ItemType Directory -Path (Join-Path $run.FullName 'source')
$null = New-Item -ItemType Directory -Path (Join-Path $run.FullName 'dest')
$reports = New-Item -ItemType Directory -Path (Join-Path $run.FullName 'reports')
Set-Content -LiteralPath (Join-Path $source.FullName 'note.txt') -Value 'Firewol disposable interface fixture.'
Set-Content -LiteralPath (Join-Path $source.FullName 'file2.txt') -Value 'two'
Set-Content -LiteralPath (Join-Path $source.FullName 'file10.txt') -Value 'ten'
$folder = New-Item -ItemType Directory -Path (Join-Path $source.FullName 'z-folder')
# Search steps: a nested 4 KB log, and enough nested matches for more than two result pages.
[IO.File]::WriteAllText((Join-Path $folder.FullName 'report-2026.log'), ('x' * 4096))
$many = New-Item -ItemType Directory -Path (Join-Path $folder.FullName 'many')
0..299 | ForEach-Object { [IO.File]::WriteAllText((Join-Path $many.FullName ('item-{0:000}.txt' -f $_)), '') }
$variables = @{
    LOCALAPPDATA = Join-Path $run.FullName 'local'
    WEBVIEW2_USER_DATA_FOLDER = Join-Path $run.FullName 'webview'
    FIREWOL_OPEN = $source.FullName
    FIREWOL_SPIKE_REPORT = $reports.FullName
    FIREWOL_SELF_TEST = '1'
}
$saved = @{}
$process = $null
try {
    foreach ($name in $variables.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $variables[$name])
    }
    # The elevated NTFS helper must exit before Tauri starts: no window and no WebView2 profile.
    $helper = Start-Process -FilePath $appPath -ArgumentList '--ntfs-helper', 'C', '1', 'not-a-firewol-pipe', "$PID" -WindowStyle Hidden -PassThru
    if (-not $helper.WaitForExit(10000)) { $helper.Kill($true); throw '--ntfs-helper started the interface instead of exiting' }
    if ($helper.ExitCode -eq 0) { throw '--ntfs-helper reported success without its pipe' }
    if (Test-Path -LiteralPath $variables.WEBVIEW2_USER_DATA_FOLDER) { throw '--ntfs-helper created a WebView2 profile' }
    Write-Output "--ntfs-helper exited with code $($helper.ExitCode) before the interface started"
    $process = Start-Process -FilePath $appPath -WindowStyle Hidden -PassThru
    $report = Join-Path $reports.FullName 'main.json'
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $report)) {
        if ($process.HasExited) { throw 'Firewol exited before reporting' }
        if ($timer.Elapsed.TotalSeconds -gt 120) { throw "Interface test timed out: $($run.FullName)" }
        Start-Sleep -Milliseconds 100
    }
    $result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    $result | ConvertTo-Json -Depth 6
    Write-Output "Report: $report"
    if (-not $result.passed) { throw 'Interface self-test failed' }
    # The detached tab reports from its own window once it has listed its folder.
    $adopted = $null
    while (-not $adopted) {
        $adopted = Get-ChildItem -LiteralPath $reports.FullName -Filter 'window-*.json' |
            ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json } |
            Where-Object { $_.ready -and $_.adopted -eq 'folder' -and $_.path } | Select-Object -First 1
        if ($adopted) { break }
        if ($timer.Elapsed.TotalSeconds -gt 120) { throw 'The detached tab never reported from its new window' }
        Start-Sleep -Milliseconds 100
    }
    Write-Output "Detached tab listed $($adopted.path) in $($adopted.label)"
} finally {
    if ($process -and -not $process.HasExited) { $process.Kill($true); $null = $process.WaitForExit(5000) }
    foreach ($name in $saved.Keys) {
        if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
    }
}
