param(
    [Parameter(Mandatory)] [string] $App,
    [Parameter(Mandatory)] [string] $Fixture
)

$ErrorActionPreference = 'Stop'
$appPath = (Resolve-Path -LiteralPath $App).Path
$fixturePath = (Resolve-Path -LiteralPath $Fixture).Path
$cli = Join-Path (Split-Path -Parent $appPath) 'firewol-m0.exe'
$info = Get-Content -LiteralPath (Join-Path $fixturePath 'fixture.json') -Raw | ConvertFrom-Json
$run = New-Item -ItemType Directory -Path (Join-Path $PSScriptRoot ('../target/measure-' + [guid]::NewGuid().ToString('N')))
$report = Join-Path $run.FullName 'frontend.json'

. (Join-Path $PSScriptRoot 'PrivateWorkingSet.ps1')

$phases = [ordered]@{}
$oldRoot = $env:FIREWOL_M0_ROOT
$oldReport = $env:FIREWOL_M0_REPORT
$oldWebView = $env:WEBVIEW2_USER_DATA_FOLDER
$appProcess = $null
$helper = $null

function Measure-Phase([string] $Name, [scriptblock] $Done, [int] $TimeoutSeconds) {
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $peak = 0
    $samples = 0
    do {
        if ($appProcess.HasExited) { throw 'The Firewol window closed before measurement finished' }
        $ids = @($appProcess.Id)
        if ($helper -and -not $helper.HasExited) { $ids += $helper.Id }
        $bytes = [FirewolM0Memory]::Measure([uint32[]]$ids)
        $peak = [Math]::Max([uint64]$peak, $bytes)
        $samples++
        if (& $Done) { break }
        if ($timer.Elapsed.TotalSeconds -ge $TimeoutSeconds) { throw "Timed out during $Name" }
        Start-Sleep -Milliseconds 20
    } while ($true)
    $phases[$Name] = [ordered]@{ peak_bytes = $peak; samples = $samples; elapsed_ms = $timer.ElapsedMilliseconds }
}

try {
    $env:FIREWOL_M0_ROOT = $fixturePath
    $env:FIREWOL_M0_REPORT = $report
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $run.FullName 'webview'
    $appProcess = Start-Process -FilePath $appPath -PassThru
    Measure-Phase -Name 'startup_search_scroll' -Done {
        (Test-Path -LiteralPath $report) -and (Get-Item -LiteralPath $report).Length -gt 0
    } -TimeoutSeconds 60
    $ui = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    $idle = [System.Diagnostics.Stopwatch]::StartNew()
    Measure-Phase -Name 'idle' -Done { $idle.Elapsed.TotalSeconds -ge 3 } -TimeoutSeconds 10

    $helper = Start-Process -FilePath $cli -ArgumentList 'index-load', $info.index_rows -WindowStyle Hidden -PassThru
    Measure-Phase -Name 'indexing' -Done { $helper.HasExited } -TimeoutSeconds 180
    if ($helper.ExitCode -ne 0) { throw "Index workload exited with code $($helper.ExitCode)" }
    $helper = Start-Process -FilePath $cli -ArgumentList 'copy-load' -WindowStyle Hidden -PassThru
    Measure-Phase -Name 'transfer' -Done { $helper.HasExited } -TimeoutSeconds 30
    if ($helper.ExitCode -ne 0) { throw "Copy workload exited with code $($helper.ExitCode)" }
    $peak = ($phases.Values | ForEach-Object { $_.peak_bytes } | Measure-Object -Maximum).Maximum
    $result = [ordered]@{
        executable = Split-Path -Leaf $appPath
        fixture = $info
        ui = $ui
        phases = $phases
        peak_private_working_set_bytes = [uint64]$peak
        below_cap = $peak -lt 150000000
    }
    $result | ConvertTo-Json -Depth 6 | Tee-Object -FilePath (Join-Path $run.FullName 'result.json')
    Write-Information "Results saved to $($run.FullName)" -InformationAction Continue
} finally {
    if ($helper -and -not $helper.HasExited) { $helper.Kill() }
    if ($appProcess -and -not $appProcess.HasExited) {
        $null = $appProcess.CloseMainWindow()
        if (-not $appProcess.WaitForExit(3000)) { $appProcess.Kill() }
    }
    $env:FIREWOL_M0_ROOT = $oldRoot
    $env:FIREWOL_M0_REPORT = $oldReport
    $env:WEBVIEW2_USER_DATA_FOLDER = $oldWebView
}
