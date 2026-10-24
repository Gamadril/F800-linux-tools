# Restore a complete SPI NOR dump to an F800 board in USB FEL mode.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Image
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

$xfel = Join-Path $PSScriptRoot 'bin\xfel_spi_nor.exe'
if (-not (Test-Path -LiteralPath $xfel -PathType Leaf)) {
    throw "xfel_spi_nor not found: $xfel; extract the complete release package."
}
if (-not (Test-Path -LiteralPath $Image -PathType Leaf)) {
    throw "not a file: $Image"
}

function Invoke-Xfel {
    param([string[]]$ToolArgs)
    & $xfel @ToolArgs
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$bytes = (Get-Item -LiteralPath $Image).Length
Write-Host "==> $xfel version"
Invoke-Xfel -ToolArgs @('version')
$spinorOutput = (Invoke-Xfel -ToolArgs @('spinor') | Out-String).Trim()
Write-Host $spinorOutput
[uint64]$capacity = 0
if ($spinorOutput -notmatch '^.+\s+([0-9]+)$' -or
    -not [uint64]::TryParse($Matches[1], [ref]$capacity) -or $capacity -eq 0) {
    throw 'could not parse NOR size from device'
}
if ([uint64]$bytes -ne $capacity) {
    throw "full recovery requires exactly $capacity bytes (got $bytes)"
}

Invoke-Xfel -ToolArgs @('write', '--dry-run', '0', $Image)
$answer = Read-Host 'This will ERASE and overwrite SPI NOR. Continue? [y/N]'
if ($answer -notmatch '^(?i:y|yes)$') { exit 1 }
Invoke-Xfel -ToolArgs @('write', '--yes', '0', $Image)
Write-Host '==> done'
