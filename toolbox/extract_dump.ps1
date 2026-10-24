# Extract a SPI NOR dump into workspace/ beside this script.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Dump
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

$dumpTool = Join-Path $PSScriptRoot 'bin\dump_tool.exe'
$workspace = Join-Path $PSScriptRoot 'workspace'
if (-not (Test-Path -LiteralPath $dumpTool -PathType Leaf)) {
    throw "dump_tool not found: $dumpTool; extract the complete release package."
}
if (-not (Test-Path -LiteralPath $Dump -PathType Leaf)) {
    throw "not a file: $Dump"
}
if (Test-Path -LiteralPath $workspace) {
    $answer = Read-Host 'workspace exists. Replace? [y/N]'
    if ($answer -notmatch '^(?i:y|yes)$') { exit 1 }
    Remove-Item -LiteralPath $workspace -Recurse -Force
}

Write-Host "==> extract $Dump -> $workspace"
& $dumpTool extract -v $Dump $workspace
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host '==> done'
