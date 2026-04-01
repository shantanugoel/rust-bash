#!/usr/bin/env bash
set -euo pipefail

# Publish rust-bash to npm using bun.
#
# This script handles the FULL build pipeline:
#   1. Build WASM binary (Rust → wasm-bindgen → packages/core/wasm/)
#   2. Build native Node.js addon (Rust → napi-rs → packages/core/native/)
#   3. Compile TypeScript (tsc → packages/core/dist/)
#   4. Run tests
#   5. Publish to npm
#
# Version bumping is handled separately by scripts/version.sh.
# Workflow: version.sh → commit → publish-npm.sh
#
# Usage:
#   ./scripts/publish-npm.sh              # build everything + publish
#   ./scripts/publish-npm.sh --dry-run    # build + preview what would be published
#   ./scripts/publish-npm.sh --skip-rust  # skip Rust builds (use existing artifacts)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PKG_DIR="$REPO_ROOT/packages/core"
HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"

DRY_RUN=false
SKIP_RUST=false

for arg in "$@"; do
  case "$arg" in
    --dry-run)    DRY_RUN=true ;;
    --skip-rust)  SKIP_RUST=true ;;
    *)
      echo "Unknown argument: $arg"
      echo "Usage: $0 [--dry-run] [--skip-rust]"
      exit 1
      ;;
  esac
done

# --- Pre-flight checks ---

if ! command -v bun &>/dev/null; then
  echo "Error: bun is not installed. Install it from https://bun.sh"
  exit 1
fi

cd "$PKG_DIR"
echo "📦 Package: $(jq -r .name package.json) v$(jq -r .version package.json)"

# Check npm auth
if ! npm whoami &>/dev/null; then
  echo "Error: not logged in to npm. Run 'npm login' first."
  exit 1
fi
echo "👤 Logged in as: $(npm whoami)"

# Check for uncommitted changes
if ! git -C "$REPO_ROOT" diff --quiet HEAD 2>/dev/null; then
  echo ""
  echo "⚠️  Warning: uncommitted changes in repo"
  git -C "$REPO_ROOT" --no-pager diff --stat HEAD
  echo ""
  read -p "Continue anyway? [y/N] " -n 1 -r
  echo
  [[ $REPLY =~ ^[Yy]$ ]] || exit 1
fi

# Verify versions are in sync
bash "$SCRIPT_DIR/check-version-sync.sh"

# ─── Step 1: Build WASM ──────────────────────────────────────────────

if ! $SKIP_RUST; then
  echo ""
  echo "🦀 Step 1/4: Building WASM binary..."
  bash "$SCRIPT_DIR/build-wasm.sh"

  # Copy WASM artifacts from pkg/ → packages/core/wasm/
  echo "   Copying WASM artifacts to packages/core/wasm/..."
  mkdir -p "$PKG_DIR/wasm"
  cp "$REPO_ROOT/pkg/rust_bash.js"         "$PKG_DIR/wasm/"
  cp "$REPO_ROOT/pkg/rust_bash_bg.wasm"    "$PKG_DIR/wasm/"
  cp "$REPO_ROOT/pkg/rust_bash.d.ts"       "$PKG_DIR/wasm/" 2>/dev/null || true
  cp "$REPO_ROOT/pkg/rust_bash_bg.wasm.d.ts" "$PKG_DIR/wasm/" 2>/dev/null || true
  echo "   ✅ WASM artifacts ready"

  # ─── Step 2: Build native addon ──────────────────────────────────────

  echo ""
  echo "🦀 Step 2/4: Building native Node.js addon..."
  rm -f "$PKG_DIR/native"/rust-bash-native.*.node
  cargo build \
    --manifest-path "$PKG_DIR/native/Cargo.toml" \
    --release \
    --target "$HOST_TARGET"

  NODE_FILE="$(bash "$SCRIPT_DIR/stage-native-addon.sh" "$HOST_TARGET" "$PKG_DIR/native")"
  NODE_SIZE=$(wc -c < "$NODE_FILE")
  echo "   ✅ Native addon ready ($(( NODE_SIZE / 1024 )) KB)"

else
  echo ""
  echo "⏭️  Skipping Rust builds (--skip-rust)"

  # Verify artifacts exist
  if [[ ! -f "$PKG_DIR/wasm/rust_bash_bg.wasm" ]]; then
    echo "Error: WASM artifacts missing in packages/core/wasm/. Run without --skip-rust."
    exit 1
  fi
  if ! compgen -G "$PKG_DIR/native/rust-bash-native.*.node" > /dev/null; then
    echo "Warning: native addon missing in packages/core/native/. Package will be WASM-only."
  fi
fi

# ─── Step 3: Compile TypeScript ────────────────────────────────────────

echo ""
echo "📘 Step 3/4: Compiling TypeScript..."
cd "$PKG_DIR"
bun run build
echo "   ✅ TypeScript compiled"

# ─── Step 4: Run tests ────────────────────────────────────────────────

echo ""
echo "🧪 Step 4/4: Running tests..."
bun run test
echo "   ✅ Tests passed"

# ─── Publish ──────────────────────────────────────────────────────────

CURRENT_VERSION=$(jq -r .version package.json)

if $DRY_RUN; then
  echo ""
  echo "🔍 Dry run — previewing package contents:"
  bun pm pack --dry-run
  echo ""
  echo "Would publish rust-bash@$CURRENT_VERSION"
else
  echo ""
  echo "🚀 Publishing rust-bash@$CURRENT_VERSION..."
  bun publish --access public
  echo ""
  echo "✅ Published! https://www.npmjs.com/package/rust-bash"
fi
