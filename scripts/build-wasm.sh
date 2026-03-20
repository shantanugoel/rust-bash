#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Building rust-bash for WASM ==="

# Check for wasm32 target
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "Installing wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

# Check for wasm-bindgen-cli
if ! command -v wasm-bindgen &>/dev/null; then
    echo "Installing wasm-bindgen-cli..."
    cargo install wasm-bindgen-cli
fi

# Build
echo "Building with cargo..."
cargo build \
    --manifest-path "$PROJECT_DIR/Cargo.toml" \
    --target wasm32-unknown-unknown \
    --features wasm \
    --no-default-features \
    --release

# Run wasm-bindgen
OUT_DIR="${PROJECT_DIR}/pkg"
mkdir -p "$OUT_DIR"

echo "Running wasm-bindgen..."
wasm-bindgen \
    "${PROJECT_DIR}/target/wasm32-unknown-unknown/release/rust_bash.wasm" \
    --out-dir "$OUT_DIR" \
    --target web

# Optional: wasm-opt for size optimization
if command -v wasm-opt &>/dev/null; then
    echo "Running wasm-opt..."
    wasm-opt "$OUT_DIR/rust_bash_bg.wasm" -Oz -o "$OUT_DIR/rust_bash_bg.wasm"
fi

# Copy WASM binary to website public/ for dev mode serving
WEBSITE_PUBLIC="${PROJECT_DIR}/examples/website/public"
if [ -d "$WEBSITE_PUBLIC" ]; then
    cp "$OUT_DIR/rust_bash_bg.wasm" "$WEBSITE_PUBLIC/rust_bash_bg.wasm"
    echo "Copied WASM binary to website public/"
fi

# Report size
WASM_SIZE=$(wc -c < "$OUT_DIR/rust_bash_bg.wasm")
WASM_SIZE_KB=$((WASM_SIZE / 1024))
echo "=== WASM build complete ==="
echo "Output: $OUT_DIR/"
echo "Binary size: ${WASM_SIZE_KB} KB"
