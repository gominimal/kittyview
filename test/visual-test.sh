#!/usr/bin/env bash
# Visual regression test for kittyview SVG text rendering.
#
# Run this in a kitty-graphics-capable terminal (kitty, ghostty, wezterm, etc.)
# to verify that text renders correctly in SVG-based images.
#
# Usage:
#   ./test/visual-test.sh [path-to-kittyview]
#
# The script displays several test images and asks for visual confirmation.

set -euo pipefail

KITTYVIEW="${1:-cargo run --release --}"
PASS=0
FAIL=0
SKIP=0

# Create a temp directory for SVG test files (macOS mktemp doesn't support suffixes)
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

ask_yn() {
    local prompt="$1"
    while true; do
        printf "${BOLD}%s [y/n]: ${RESET}" "$prompt"
        read -r -n 1 answer
        echo
        case "$answer" in
            [yY]) return 0 ;;
            [nN]) return 1 ;;
            *) echo "Please answer y or n." ;;
        esac
    done
}

run_test() {
    local name="$1"
    local description="$2"
    shift 2

    echo
    printf "${BOLD}=== Test: %s ===${RESET}\n" "$name"
    echo "$description"
    echo

    if ! eval "$@" 2>/dev/null; then
        printf "${YELLOW}SKIP${RESET} - command failed (missing dependencies or unsupported terminal)\n"
        SKIP=$((SKIP + 1))
        return
    fi

    echo
    if ask_yn "Does the image above look correct?"; then
        printf "${GREEN}PASS${RESET}\n"
        PASS=$((PASS + 1))
    else
        printf "${RED}FAIL${RESET}\n"
        FAIL=$((FAIL + 1))
    fi
}

# ── Test 1: Animated logo (SVG text in speech bubble) ──────────

run_test "animated-logo" \
    "You should see a pixel-art kitten with a white speech bubble.
The bubble should contain:
  - \"kittyview\" in bold dark purple text
  - \"github.com/gominimal/kittyview\" in lighter purple text below" \
    "$KITTYVIEW logo --animate"

# ── Test 2: Static logo (pixel art, no SVG text) ──────────────

run_test "static-logo" \
    "You should see a small pixel-art kitten face (purple/pink).
No text expected - this tests basic kitty graphics protocol." \
    "$KITTYVIEW logo"

# ── Test 3: SVG with text (generated inline) ──────────────────

SVG_TEXT_FILE="$TMPDIR_TEST/text-test.svg"
cat > "$SVG_TEXT_FILE" << 'SVGEOF'
<svg xmlns="http://www.w3.org/2000/svg" width="400" height="120">
  <rect width="400" height="120" rx="8" fill="#f0f0f0"/>
  <text x="200" y="40" text-anchor="middle" font-size="24" fill="#333" font-family="sans-serif" font-weight="bold">SVG Text Test</text>
  <text x="200" y="70" text-anchor="middle" font-size="16" fill="#666" font-family="sans-serif">If you can read this, fonts work!</text>
  <text x="200" y="100" text-anchor="middle" font-size="14" fill="#999" font-family="monospace">monospace: 0123456789</text>
</svg>
SVGEOF

run_test "svg-text" \
    "You should see a light gray box containing three lines of text:
  - \"SVG Text Test\" (bold, dark)
  - \"If you can read this, fonts work!\" (medium gray)
  - \"monospace: 0123456789\" (light gray, monospace font)" \
    "$KITTYVIEW '$SVG_TEXT_FILE'"

# ── Test 4: Mermaid-style SVG with foreignObject ─────────────

SVG_FOREIGN_FILE="$TMPDIR_TEST/foreign-test.svg"
cat > "$SVG_FOREIGN_FILE" << 'SVGEOF'
<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">
  <style>font-size:16px;</style>
  <rect x="50" y="30" width="200" height="60" rx="8" fill="#ECECFF" stroke="#9370DB"/>
  <g transform="translate(100, 45)">
    <foreignObject width="100" height="30">
      <div xmlns="http://www.w3.org/1999/xhtml">
        <span class="nodeLabel"><p>Node A</p></span>
      </div>
    </foreignObject>
  </g>
  <rect x="50" y="120" width="200" height="60" rx="8" fill="#ECECFF" stroke="#9370DB"/>
  <g transform="translate(100, 135)">
    <foreignObject width="100" height="30">
      <div xmlns="http://www.w3.org/1999/xhtml">
        <span class="nodeLabel"><p>Node B</p></span>
      </div>
    </foreignObject>
  </g>
  <line x1="150" y1="90" x2="150" y2="120" stroke="#333" stroke-width="2" marker-end="url(#arrow)"/>
</svg>
SVGEOF

run_test "foreignObject-text" \
    "You should see two purple rounded boxes stacked vertically:
  - Top box: text \"Node A\" centered
  - Bottom box: text \"Node B\" centered
  - A line connecting them
(Note: text is converted from foreignObject -- tests the mermaid conversion pipeline)" \
    "$KITTYVIEW '$SVG_FOREIGN_FILE'"

# ── Test 5: SVG with <br/> in foreignObject ──────────────────

SVG_BR_FILE="$TMPDIR_TEST/br-test.svg"
cat > "$SVG_BR_FILE" << 'SVGEOF'
<svg xmlns="http://www.w3.org/2000/svg" width="300" height="120">
  <style>font-size:14px;</style>
  <rect x="50" y="20" width="200" height="80" rx="8" fill="#E8F5E9" stroke="#4CAF50"/>
  <g transform="translate(100, 30)">
    <foreignObject width="100" height="60">
      <div xmlns="http://www.w3.org/1999/xhtml">
        <p>line one<br/>line two<br/>line three</p>
      </div>
    </foreignObject>
  </g>
</svg>
SVGEOF

run_test "br-tag-multiline" \
    "You should see a green rounded box containing THREE lines of text:
  - \"line one\"
  - \"line two\"
  - \"line three\"
(Tests <br/> handling in foreignObject conversion)" \
    "$KITTYVIEW '$SVG_BR_FILE'"

# ── Summary ──────────────────────────────────────────────────

echo
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
printf "${BOLD}Results:${RESET} "
printf "${GREEN}%d passed${RESET}, " "$PASS"
printf "${RED}%d failed${RESET}, " "$FAIL"
printf "${YELLOW}%d skipped${RESET}\n" "$SKIP"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ "$FAIL" -gt 0 ]; then
    echo
    echo "If text is missing from SVG images, possible causes:"
    echo "  - No sans-serif font installed (need one of: Liberation Sans,"
    echo "    DejaVu Sans, Helvetica, Arial)"
    echo "  - Terminal does not support kitty graphics protocol"
    echo "  - Running inside tmux without 'set -g allow-passthrough on'"
    exit 1
fi
