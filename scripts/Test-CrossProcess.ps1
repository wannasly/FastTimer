param(
    [string]$Bin = (Join-Path $PSScriptRoot '..\target\release'),
    [string]$Out = (Join-Path $PSScriptRoot '..\target\cross-process'),
    [ValidateRange(1, 10000)][int]$Samples = 120
)

$ErrorActionPreference = 'Stop'
$gui = (Resolve-Path -LiteralPath (Join-Path $Bin 'fasttimer.exe')).Path
$cli = (Resolve-Path -LiteralPath (Join-Path $Bin 'fasttimer-cli.exe')).Path
$session = (Get-Process -Id $PID).SessionId
if (Get-Process -Name fasttimer -ErrorAction SilentlyContinue | Where-Object SessionId -eq $session) {
    throw 'Close the existing FastTimer desktop instance before this test.'
}
$folder = (New-Item -ItemType Directory -Force -Path $Out).FullName

function Measure-Phase([string]$Phase, [string]$Cases) {
    $json = Join-Path $folder "$Phase.json"
    $csv = Join-Path $folder "$Phase.csv"
    & $cli benchmark --cases $Cases --samples $Samples --rounds 3 --json $json --csv $csv
    if ($LASTEXITCODE -ne 0) { throw "Measurement failed in $Phase." }
}

function Measure-WithHolder([string]$Phase, [string]$Request, [bool]$Hidden) {
    $arguments = @('--request', $Request)
    if ($Hidden) { $arguments += '--hidden' }
    $style = if ($Hidden) { 'Hidden' } else { 'Normal' }
    $process = Start-Process -FilePath $gui -ArgumentList $arguments -WindowStyle $style -PassThru
    try {
        if (-not $process.WaitForInputIdle(5000)) { throw "Holder did not initialize in $Phase." }
        $process.Refresh()
        if ($process.HasExited -or $Hidden -ne ($process.MainWindowHandle -eq [IntPtr]::Zero)) {
            throw "Holder visibility did not match the test in $Phase."
        }
        Measure-Phase $Phase 'baseline'
    } finally {
        if (-not $process.HasExited) {
            $exit = Start-Process -FilePath $gui -ArgumentList '--exit' -WindowStyle Hidden -Wait -PassThru
            if ($exit.ExitCode -ne 0 -or -not $process.WaitForExit(5000)) {
                $process.Kill()
                throw "Holder did not exit cleanly in $Phase."
            }
        }
    }
}

Measure-Phase 'before' 'baseline'
Measure-Phase 'own-request' 'baseline,fastest,1.0000,winmm:1'
Measure-WithHolder 'external-native-visible' 'fastest' $false
Measure-WithHolder 'external-native-hidden' 'fastest' $true
Measure-WithHolder 'external-winmm-hidden' 'winmm:1' $true
Measure-Phase 'after' 'baseline'
