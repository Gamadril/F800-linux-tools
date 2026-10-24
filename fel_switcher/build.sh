#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

TARGET=armv7a-none-eabi
HOST=$(rustc -vV | sed -n 's/^host: //p')
ELF=target/${TARGET}/release/fel_switcher
EGON_PAD=target/${HOST}/release/egon_pad
BIN=target/fel_switcher.bin

build() {
  cargo build --locked --release --bin fel_switcher
  cargo build --locked --release --bin egon_pad --target "${HOST}"
  "${EGON_PAD}" "${ELF}" "${BIN}"
}

clean() {
  read -r -p 'Remove generated FEL build artifacts? [y/N] ' answer
  [[ $answer == [yY] || $answer == [yY][eE][sS] ]] || return 1
  cargo clean
  rm -f "${BIN}"
}

case "${1:-build}" in
  build) build ;;
  clean) clean ;;
  *)
    echo "usage: $0 [build|clean]" >&2
    exit 1
    ;;
esac
