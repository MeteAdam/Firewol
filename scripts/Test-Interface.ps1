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
$null = New-Item -ItemType Directory -Path (Join-Path $source.FullName 'z-folder')
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
} finally {
    if ($process -and -not $process.HasExited) { $process.Kill($true); $null = $process.WaitForExit(5000) }
    foreach ($name in $saved.Keys) {
        if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
    }
}
