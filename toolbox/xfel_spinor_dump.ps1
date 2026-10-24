# Dump the complete SPI NOR from an F800 board in USB FEL mode.
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Out = ('nor_dump_{0:yyyyMMdd}.bin' -f (Get-Date))
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

$xfel = Join-Path $PSScriptRoot 'bin\xfel_spi_nor.exe'
if (-not (Test-Path -LiteralPath $xfel -PathType Leaf)) {
    throw "xfel_spi_nor not found: $xfel; extract the complete release package."
}
if (Test-Path -LiteralPath $Out) {
    throw "output already exists: $Out"
}

function Invoke-Xfel {
    param([string[]]$ToolArgs)
    & $xfel @ToolArgs
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "==> $xfel version"
Invoke-Xfel -ToolArgs @('version')
Write-Host "==> $xfel spinor"
$spinorOutput = (Invoke-Xfel -ToolArgs @('spinor') | Out-String).Trim()
Write-Host $spinorOutput
[uint64]$capacity = 0
if ($spinorOutput -notmatch '^.+\s+([0-9]+)$' -or
    -not [uint64]::TryParse($Matches[1], [ref]$capacity) -or $capacity -eq 0) {
    throw 'could not parse NOR size from device'
}

Write-Host "==> read $capacity bytes @ 0 -> $Out"
Invoke-Xfel -ToolArgs @('read', '0', [string]$capacity, $Out)
if ([uint64](Get-Item -LiteralPath $Out).Length -ne $capacity) {
    throw 'dump length does not match detected NOR capacity'
}
Write-Host "==> done: $Out"
