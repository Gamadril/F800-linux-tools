#!/usr/bin/env bash
# Write SPI NOR image via xfel_spi_nor (V851S/V853 device in FEL mode, USB connected).
#
# Usage:
#   ./xfel_spinor_recovery.sh <flash_dump.bin>
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

[[ $# -eq 1 ]] || { echo "usage: $0 <flash_dump.bin>" >&2; exit 1; }
IMG=$1
[[ -f ${IMG} ]] || { echo "not a file: ${IMG}" >&2; exit 1; }

run_xfel() {
  if [[ ${EUID} -eq 0 || ${OSTYPE} == msys* || ${OSTYPE} == cygwin* ]]; then
    "${XFEL}" "$@"
  else
    sudo "${XFEL}" "$@"
  fi
}

bytes=$(wc -c <"${IMG}" | tr -d ' ')
echo "==> ${XFEL} version"
run_xfel version
capacity=$(run_xfel spinor | awk '{print $NF}')
[[ $bytes == "$capacity" ]] || { echo "full recovery requires exactly ${capacity} bytes (got ${bytes})" >&2; exit 1; }
run_xfel write --dry-run 0 "${IMG}"
echo "==> write ${bytes} bytes @ 0 <- ${IMG}"
read -r -p "This will ERASE and overwrite SPI NOR. Continue? [y/N] " ans
[[ ${ans} == [yY] || ${ans} == [yY][eE][sS] ]] || exit 1
run_xfel write --yes 0 "${IMG}"
echo "==> done"
