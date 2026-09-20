#!/usr/bin/env bash
# Fails if the committed TypeScript types no longer match the Rust.
#
# Drifting IPC payload shapes are the most common bug in a Tauri app and the
# hardest to spot: both sides compile, and the mismatch only shows at runtime.
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/gen-types.sh >/dev/null

if ! git diff --quiet -- src/types; then
    echo "generated types are out of date:" >&2
    git --no-pager diff --stat -- src/types >&2
    echo >&2
    echo "run ./scripts/gen-types.sh and commit the result" >&2
    exit 1
fi

# A newly added type is untracked rather than modified, so check for that too.
if [ -n "$(git ls-files --others --exclude-standard -- src/types)" ]; then
    echo "generated types include files that are not committed:" >&2
    git ls-files --others --exclude-standard -- src/types >&2
    echo >&2
    echo "run ./scripts/gen-types.sh and commit the result" >&2
    exit 1
fi

echo "generated types are up to date"
