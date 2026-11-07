param(
    [Parameter(Mandatory)] [int] $AppProcessId,
    [Parameter(Mandatory)] [string] $ControlPath,
    [Parameter(Mandatory)] [string] $OutputPath,
    [ValidateRange(1, 3600)] [int] $TimeoutSeconds = 900
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'PrivateWorkingSet.ps1')
$null = Get-Process -Id $AppProcessId -ErrorAction Stop
$phases = [ordered]@{}
$timer = [System.Diagnostics.Stopwatch]::StartNew()
$previous = 0L
$largestGap = 0L
$timedOut = $true
$phase = 'startup'

Write-Output "Sampling process $AppProcessId; control: $ControlPath"
try {
    while ($timer.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        $control = Get-Content -LiteralPath $ControlPath -Raw | ConvertFrom-Json
        if ($control.stop) {
            $timedOut = $false
            break
        }
        $null = Get-Process -Id $AppProcessId -ErrorAction Stop
        $phase = [string]$control.phase
        if ([string]::IsNullOrWhiteSpace($phase)) { throw 'Control phase must not be empty' }
        if (-not $phases.Contains($phase)) {
            $phases[$phase] = [ordered]@{ peak_bytes = 0L; samples = 0; max_process_roots = 0 }
            Write-Output "Sampling phase: $phase"
        }
        $roots = [uint32[]](@($AppProcessId) + @($control.process_ids))
        $bytes = [FirewolM0Memory]::Measure($roots)
        $sample = $phases[$phase]
        $sample.peak_bytes = [Math]::Max([uint64]$sample.peak_bytes, $bytes)
        $sample.samples++
        $sample.max_process_roots = [Math]::Max($sample.max_process_roots, $roots.Count)
        $now = $timer.ElapsedMilliseconds
        if ($previous -gt 0) { $largestGap = [Math]::Max($largestGap, $now - $previous) }
        $previous = $now
        Start-Sleep -Milliseconds 20
    }
} finally {
    $peak = ($phases.Values | ForEach-Object { $_.peak_bytes } | Measure-Object -Maximum).Maximum
    $sampleCount = ($phases.Values | ForEach-Object { $_.samples } | Measure-Object -Sum).Sum
    [ordered]@{
        app_process_id = $AppProcessId
        phases = $phases
        peak_private_working_set_bytes = [uint64]$peak
        below_cap = $sampleCount -gt 0 -and $peak -lt 150000000
        sample_count = [int]$sampleCount
        largest_sample_gap_ms = $largestGap
        elapsed_ms = $timer.ElapsedMilliseconds
        timed_out = $timedOut
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding utf8
}
if ($timedOut) { throw "Session measurement timed out after $TimeoutSeconds seconds" }
if ($sampleCount -eq 0) { throw 'No memory samples were collected' }
Get-Content -LiteralPath $OutputPath
