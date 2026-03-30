#!/usr/bin/env bash
set -euo pipefail

# Verify that all package versions are in sync.
# Used by publish-npm.sh and suitable for CI.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CARGO_VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
NATIVE_VERSION=$(grep '^version' "$REPO_ROOT/packages/core/native/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
NPM_VERSION=$(jq -r .version "$REPO_ROOT/packages/core/package.json")

PASS=true

if [[ "$CARGO_VERSION" != "$NPM_VERSION" ]]; then
  echo "❌ Version mismatch: Cargo.toml ($CARGO_VERSION) ≠ package.json ($NPM_VERSION)"
  PASS=false
fi

if [[ "$CARGO_VERSION" != "$NATIVE_VERSION" ]]; then
  echo "❌ Version mismatch: Cargo.toml ($CARGO_VERSION) ≠ native/Cargo.toml ($NATIVE_VERSION)"
  PASS=false
fi

if $PASS; then
  echo "✅ All versions in sync: $CARGO_VERSION"
else
  echo ""
  echo "Run ./scripts/version.sh <patch|minor|major> to fix."
  exit 1
fi
