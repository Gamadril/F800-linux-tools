#!/usr/bin/env bash
# Extract SPI NOR dump into ./workspace/ using dump_tool.
#
# Usage:
#   ./extract_dump.sh <dump.bin>
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
if [[ -x "${SCRIPT_DIR}/bin/dump_tool" ]]; then
  DUMP_TOOL="${SCRIPT_DIR}/bin/dump_tool"
elif [[ -f "${SCRIPT_DIR}/bin/dump_tool.exe" ]]; then
  DUMP_TOOL="${SCRIPT_DIR}/bin/dump_tool.exe"
elif [[ -x "${SCRIPT_DIR}/../target/release/dump_tool" ]]; then
  DUMP_TOOL="${SCRIPT_DIR}/../target/release/dump_tool"
elif [[ -f "${SCRIPT_DIR}/../target/release/dump_tool.exe" ]]; then
  DUMP_TOOL="${SCRIPT_DIR}/../target/release/dump_tool.exe"
else
  echo "dump_tool not found (run: cargo build --release)" >&2
  exit 1
fi
WORKSPACE=${SCRIPT_DIR}/workspace

[[ $# -eq 1 ]] || { echo "usage: $0 <dump.bin>" >&2; exit 1; }
DUMP=$1
[[ -f ${DUMP} ]] || { echo "not a file: ${DUMP}" >&2; exit 1; }

if [[ -e ${WORKSPACE} ]]; then
  read -r -p "workspace exists. Replace? [y/N] " ans
  [[ ${ans} == [yY] || ${ans} == [yY][eE][sS] ]] || exit 1
  rm -rf "${WORKSPACE}"
fi

echo "==> extract ${DUMP} -> ${WORKSPACE}"
"${DUMP_TOOL}" extract -v "${DUMP}" "${WORKSPACE}"
echo "==> done"
