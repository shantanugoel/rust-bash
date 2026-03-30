#!/usr/bin/env bash
set -euo pipefail

# Bump the version across all packages atomically.
# Updates: Cargo.toml, packages/core/native/Cargo.toml, packages/core/package.json
#
# Usage:
#   ./scripts/version.sh patch    # 0.1.0 → 0.1.1
#   ./scripts/version.sh minor    # 0.1.0 → 0.2.0
#   ./scripts/version.sh major    # 0.1.0 → 1.0.0
#   ./scripts/version.sh 1.2.3    # set explicit version

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CARGO_TOML="$REPO_ROOT/Cargo.toml"
NATIVE_TOML="$REPO_ROOT/packages/core/native/Cargo.toml"
PACKAGE_JSON="$REPO_ROOT/packages/core/package.json"

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <patch|minor|major|X.Y.Z>"
  exit 1
fi

BUMP="$1"

# Read current version from Cargo.toml (source of truth)
CURRENT=$(grep '^version' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP" in
  patch) NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))" ;;
  minor) NEW_VERSION="$MAJOR.$((MINOR + 1)).0" ;;
  major) NEW_VERSION="$((MAJOR + 1)).0.0" ;;
  *.*.*)  NEW_VERSION="$BUMP" ;;
  *)
    echo "Error: argument must be patch, minor, major, or X.Y.Z"
    exit 1
    ;;
esac

echo "📝 Bumping version: $CURRENT → $NEW_VERSION"

# Update Cargo.toml (first version = line only)
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW_VERSION\"/" "$CARGO_TOML"

# Update native Cargo.toml
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW_VERSION\"/" "$NATIVE_TOML"

# Update package.json
cd "$REPO_ROOT/packages/core"
npm version "$NEW_VERSION" --no-git-tag-version --allow-same-version

# Verify all match
echo ""
echo "   Cargo.toml:        $(grep '^version' "$CARGO_TOML" | head -1)"
echo "   native/Cargo.toml: $(grep '^version' "$NATIVE_TOML" | head -1)"
echo "   package.json:      v$(jq -r .version "$PACKAGE_JSON")"
echo ""
echo "✅ All versions updated to $NEW_VERSION"
echo "   Next: commit the changes, then run ./scripts/publish-npm.sh"
