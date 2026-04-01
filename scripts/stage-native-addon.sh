#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "Usage: $0 <target-triple> [native-dir]" >&2
  exit 1
fi

TARGET="$1"
NATIVE_DIR="${2:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../packages/core/native" && pwd)}"

case "$TARGET" in
  x86_64-unknown-linux-gnu)
    LIB_EXT="so"
    OUTPUT_NAME="rust-bash-native.linux-x64-gnu.node"
    ;;
  aarch64-unknown-linux-gnu)
    LIB_EXT="so"
    OUTPUT_NAME="rust-bash-native.linux-arm64-gnu.node"
    ;;
  x86_64-apple-darwin)
    LIB_EXT="dylib"
    OUTPUT_NAME="rust-bash-native.darwin-x64.node"
    ;;
  aarch64-apple-darwin)
    LIB_EXT="dylib"
    OUTPUT_NAME="rust-bash-native.darwin-arm64.node"
    ;;
  *)
    echo "Error: unsupported native packaging target: $TARGET" >&2
    exit 1
    ;;
esac

TARGET_LIB="$NATIVE_DIR/target/$TARGET/release/librust_bash_native.$LIB_EXT"
HOST_LIB="$NATIVE_DIR/target/release/librust_bash_native.$LIB_EXT"
SOURCE_LIB="$TARGET_LIB"

if [[ ! -f "$SOURCE_LIB" ]]; then
  SOURCE_LIB="$HOST_LIB"
fi

if [[ ! -f "$SOURCE_LIB" ]]; then
  echo "Error: built native library not found for $TARGET" >&2
  echo "Looked for:" >&2
  echo "  $TARGET_LIB" >&2
  echo "  $HOST_LIB" >&2
  exit 1
fi

DEST="$NATIVE_DIR/$OUTPUT_NAME"
cp "$SOURCE_LIB" "$DEST"
echo "$DEST"
