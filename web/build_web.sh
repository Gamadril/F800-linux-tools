#!/usr/bin/env bash
# Build a browser distribution without rewriting any source files.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
WEB_OUT=${1:-"$ROOT_DIR/dist/web"}
mkdir -p "$WEB_OUT"
RUSTFLAGS="-C opt-level=z -C strip=symbols" \
  cargo build --locked --release --target wasm32-unknown-unknown -p img_tool
WASM_SRC="$ROOT_DIR/target/wasm32-unknown-unknown/release/img_tool.wasm"
cp "$WASM_SRC" "$WEB_OUT/img_tool.wasm"
cp "$ROOT_DIR/web/index.html" "$ROOT_DIR/web/style.css" "$ROOT_DIR/web/app.js" "$WEB_OUT/"
echo "Browser distribution: $WEB_OUT/index.html"
