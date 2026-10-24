#!/usr/bin/env bash
# Dump full SPI NOR via xfel_spi_nor (V851S/V853 device in FEL mode, USB connected).
#
# Usage:
#   ./xfel_spinor_dump.sh [output.bin]
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
if [[ -x "${SCRIPT_DIR}/bin/xfel_spi_nor" ]]; then
  XFEL="${SCRIPT_DIR}/bin/xfel_spi_nor"
elif [[ -f "${SCRIPT_DIR}/bin/xfel_spi_nor.exe" ]]; then
  XFEL="${SCRIPT_DIR}/bin/xfel_spi_nor.exe"
elif [[ -x "${SCRIPT_DIR}/../target/release/xfel_spi_nor" ]]; then
  XFEL="${SCRIPT_DIR}/../target/release/xfel_spi_nor"
elif [[ -f "${SCRIPT_DIR}/../target/release/xfel_spi_nor.exe" ]]; then
  XFEL="${SCRIPT_DIR}/../target/release/xfel_spi_nor.exe"
else
  echo "xfel_spi_nor not found (run: cargo build --release)" >&2
  exit 1
fi

OUT=${1:-nor_dump_$(date +%Y%m%d).bin}

run_xfel() {
  if [[ ${EUID} -eq 0 || ${OSTYPE} == msys* || ${OSTYPE} == cygwin* ]]; then
    "${XFEL}" "$@"
  else
    sudo "${XFEL}" "$@"
  fi
}

echo "==> ${XFEL} version"
run_xfel version

echo "==> ${XFEL} spinor"
spinor_out=$(run_xfel spinor)
echo "${spinor_out}"

size=$(awk '{print $NF}' <<<"${spinor_out}")
echo "==> read ${size} bytes @ 0 -> ${OUT}"
run_xfel read 0 "${size}" "${OUT}"
echo "==> done: ${OUT}"
