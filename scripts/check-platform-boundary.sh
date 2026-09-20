#!/usr/bin/env bash
# Enforces the platform boundary from docs/PLAN.md §2.6.
#
# The Linux port stays cheap only while OS-specific code is confined to
# crates/shelv-core/src/platform/. Two rules, checked here because a reviewer
# will not notice a single stray import:
#
#   1. the `windows` crate is used only under platform/
#   2. `unsafe` appears only under platform/
#
# Run: ./scripts/check-platform-boundary.sh
set -euo pipefail

cd "$(dirname "$0")/.."

CORE="crates/shelv-core/src"
PLATFORM="$CORE/platform"
status=0

report() {
    echo "boundary violation: $1" >&2
    shift
    printf '  %s\n' "$@" >&2
    status=1
}

# 1. The `windows` crate, outside platform/ and outside the shell crate.
mapfile -t win_hits < <(
    grep -rEln '^\s*use\s+windows(::|\s)|^\s*extern\s+crate\s+windows' \
        --include='*.rs' "$CORE" 2>/dev/null \
        | grep -v "^$PLATFORM/" || true
)
if [ ${#win_hits[@]} -gt 0 ]; then
    report "the 'windows' crate is imported outside $PLATFORM" "${win_hits[@]}"
fi

# 2. `unsafe` outside platform/. Matches the keyword as a word so that
#    "unsafe_code" in a lint attribute and the word in prose do not trip it.
mapfile -t unsafe_hits < <(
    grep -rEln '(^|[^[:alnum:]_])unsafe[[:space:]]+(fn|impl|block|\{)' \
        --include='*.rs' "$CORE" src-tauri/src 2>/dev/null \
        | grep -v "^$PLATFORM/" || true
)
if [ ${#unsafe_hits[@]} -gt 0 ]; then
    report "'unsafe' is used outside $PLATFORM" "${unsafe_hits[@]}"
fi

# 3. allow(unsafe_code) outside platform/, which would silently reopen rule 2.
mapfile -t allow_hits < <(
    grep -rEln 'allow\(unsafe_code\)' \
        --include='*.rs' "$CORE" src-tauri/src 2>/dev/null \
        | grep -v "^$PLATFORM/" || true
)
if [ ${#allow_hits[@]} -gt 0 ]; then
    report "allow(unsafe_code) appears outside $PLATFORM" "${allow_hits[@]}"
fi

if [ "$status" -eq 0 ]; then
    echo "platform boundary intact"
fi
exit "$status"
