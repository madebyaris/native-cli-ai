#!/usr/bin/env sh
# Builds plugin.wasm as a WASM component (needs: rustup target add wasm32-wasip2).
set -eu
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/nca_spin_plugin.wasm plugin.wasm
echo "built plugin.wasm ($(wc -c < plugin.wasm) bytes)"
