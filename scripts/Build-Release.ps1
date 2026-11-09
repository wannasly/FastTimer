param([string]$Out = 'target\distribution')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $root
try {
    $metadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $version = ($metadata.packages | Where-Object name -eq 'fasttimer').version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Expected a three-part FastTimer release version.' }
    $target = 'x86_64-pc-windows-msvc'
    $targetDir = Join-Path $root 'target'
    cargo build --release --locked --target $target --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'FastTimer compilation failed.' }
    $bin = Join-Path $targetDir "$target\release"
    $folder = (New-Item -ItemType Directory -Force -Path $Out).FullName
    $compiler = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
    if (-not $compiler) {
        $compiler = @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
        ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    }
    if (-not $compiler) { throw 'Install Inno Setup 6.3 or later in the 6.x series.' }

    $prefix = "FastTimer-$version-windows-x64"
    & $compiler "/DAppVersion=$version" "/DBinDir=$bin" "/O$folder" 'installer\FastTimer.iss'
    if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
    $setup = Join-Path $folder "$prefix-setup.exe"
    $cli = Join-Path $folder 'fasttimer-cli.exe'
    $archive = Join-Path $folder "$prefix-portable.zip"
    Copy-Item -LiteralPath (Join-Path $bin 'fasttimer-cli.exe') -Destination $cli -Force
    $portableFiles = @(
        (Join-Path $bin 'fasttimer.exe')
        (Join-Path $bin 'fasttimer-cli.exe')
        (Join-Path $root 'README.md')
        (Join-Path $root 'RESEARCH.md')
    )
    Compress-Archive -LiteralPath $portableFiles -DestinationPath $archive -Force
    @($setup, $archive, $cli) | ForEach-Object {
        $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $_).Hash.ToLowerInvariant()
        "$digest  $(Split-Path -Leaf $_)"
    } | Set-Content -LiteralPath (Join-Path $folder 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Release assets: $folder"
} finally {
    Pop-Location
}
