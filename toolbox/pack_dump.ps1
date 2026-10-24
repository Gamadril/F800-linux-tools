# Repack workspace/ beside this script into a SPI NOR dump.
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Out = ('nor_repacked_{0:yyyyMMdd}.bin' -f (Get-Date))
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest

$dumpTool = Join-Path $PSScriptRoot 'bin\dump_tool.exe'
$workspace = Join-Path $PSScriptRoot 'workspace'
if (-not (Test-Path -LiteralPath $dumpTool -PathType Leaf)) {
    throw "dump_tool not found: $dumpTool; extract the complete release package."
}
if (-not (Test-Path -LiteralPath $workspace -PathType Container)) {
    throw "workspace not found: $workspace"
}
if (Test-Path -LiteralPath $Out) {
    $answer = Read-Host "$Out exists. Replace? [y/N]"
    if ($answer -notmatch '^(?i:y|yes)$') { exit 1 }
    Remove-Item -LiteralPath $Out -Force
}

Write-Host "==> pack $workspace -> $Out"
& $dumpTool pack -v $workspace $Out
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host '==> done'
