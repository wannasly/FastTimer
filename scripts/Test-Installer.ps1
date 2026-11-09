param([Parameter(Mandatory)][string]$Installer)

$ErrorActionPreference = 'Stop'
$setup = (Resolve-Path -LiteralPath $Installer).Path
$folder = Join-Path ([IO.Path]::GetTempPath()) ("FastTimer-smoke-" + [guid]::NewGuid())
$gui = Join-Path $folder 'fasttimer.exe'
$cli = Join-Path $folder 'fasttimer-cli.exe'
$uninstall = Join-Path $folder 'unins000.exe'
$json = "$folder.json"
$csv = "$folder.csv"
try {
    $install = Start-Process -FilePath $setup -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOICONS', "/DIR=`"$folder`""
    ) -WindowStyle Hidden -Wait -PassThru
    if ($install.ExitCode -ne 0) { throw "Installer exited with $($install.ExitCode)." }
    foreach ($file in $gui, $cli, $uninstall) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing $file." }
    }
    & $cli --version
    if ($LASTEXITCODE -ne 0) { throw 'Installed CLI could not start.' }
    & $cli benchmark --cases 'baseline,fastest,winmm:1' --samples 12 --warmup 2 --rounds 1 --json $json --csv $csv
    if ($LASTEXITCODE -ne 0) { throw 'Installed CLI benchmark failed.' }
    $report = Get-Content -LiteralPath $json -Raw | ConvertFrom-Json
    $rows = @(Import-Csv -LiteralPath $csv)
    if ($report.cases.Count -ne 3 -or $rows.Length -ne 3) { throw 'Incorrect benchmark exports.' }
} finally {
    if (Test-Path -LiteralPath $uninstall -PathType Leaf) {
        $remove = Start-Process -FilePath $uninstall -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART' -WindowStyle Hidden -Wait -PassThru
        if ($remove.ExitCode -ne 0 -or (Test-Path -LiteralPath $gui) -or (Test-Path -LiteralPath $cli)) {
            throw 'Uninstall smoke test failed.'
        }
    }
    foreach ($reportFile in $json, $csv) {
        if (Test-Path -LiteralPath $reportFile) { Remove-Item -LiteralPath $reportFile }
    }
}
