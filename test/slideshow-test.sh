#!/usr/bin/env bash
# Visual test for kittyview's interactive slideshow.
#
# Run this in a kitty-graphics-capable terminal (kitty, ghostty, wezterm)
# to verify slideshow behaviour end to end -- rendering, navigation, and
# above all that the terminal is left exactly as it was found. Run it a
# second time inside tmux to exercise the passthrough paths.
#
# Usage:
#   ./test/slideshow-test.sh [path-to-kittyview]
#
# The script builds a set of test images, opens the slideshow for you to
# drive, and asks for visual confirmation afterwards.

set -euo pipefail

KITTYVIEW="${1:-cargo run --release --}"
PASS=0
FAIL=0
SKIP=0

TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

ask_yn() {
    local prompt="$1"
    while true; do
        printf "${BOLD}%s [y/n/s(kip)]: ${RESET}" "$prompt"
        read -r -n 1 answer
        echo
        case "$answer" in
            [yY]) PASS=$((PASS + 1)); return 0 ;;
            [nN]) FAIL=$((FAIL + 1)); return 1 ;;
            [sS]) SKIP=$((SKIP + 1)); return 0 ;;
            *) echo "Please answer y, n, or s." ;;
        esac
    done
}

echo "Preparing test images..."

# Three distinguishable slides: the logo, a wide SVG, and a tall SVG.
$KITTYVIEW png --logo -o "$TMPDIR_TEST/1-logo.png"

cat > "$TMPDIR_TEST/2-wide.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="300">
  <rect width="1200" height="300" fill="#2f6fdd"/>
  <text x="600" y="170" font-size="90" text-anchor="middle" fill="white">WIDE 2/4</text>
</svg>
EOF

cat > "$TMPDIR_TEST/3-tall.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="300" height="1600">
  <rect width="300" height="1600" fill="#2fae56"/>
  <text x="150" y="820" font-size="80" text-anchor="middle" fill="white">TALL 3/4</text>
</svg>
EOF

# A deliberately broken file, to exercise the error slide.
printf 'not an image' > "$TMPDIR_TEST/4-broken.png"

echo
echo "Remember what this terminal looks like right now -- prompt, scrollback,"
echo "cursor. The slideshow must give all of it back untouched."
echo
echo "In the slideshow, please exercise:"
echo "  - Right/Down/Space/n and Left/Up/Backspace/p to move back and forth"
echo "  - Home and End (Fn+Left / Fn+Right on a Mac keyboard)"
echo "  - r to force a redraw"
echo "  - Ctrl-Z to suspend (this whole script stops with it), then 'fg'"
echo "  - q (or Esc, or Ctrl-C) to quit"
echo
read -r -p "Press Enter to start the slideshow..."

$KITTYVIEW "$TMPDIR_TEST"/1-logo.png "$TMPDIR_TEST"/2-wide.svg \
    "$TMPDIR_TEST"/3-tall.svg "$TMPDIR_TEST"/4-broken.png || true

echo
ask_yn "Did slide 1 (the cat logo) render immediately, centred, with ' 1/4' in the status line?" || true
ask_yn "Did navigation show WIDE 2/4 fitted to the width and TALL 3/4 fitted to the height?" || true
ask_yn "Did slide 4 show a readable error message instead of an image?" || true
ask_yn "Did Home/End jump to the first/last slide, and did navigation stop at the ends?" || true
ask_yn "Did Ctrl-Z suspend to a working shell, and 'fg' resume with the slide redrawn?" || true
ask_yn "After quitting: is this terminal exactly as it was (prompt, scrollback, cursor, colours)?" || true
ask_yn "Type something at the prompt: no stray characters, no raw mode leftovers?" || true

if [ -n "${TMUX:-}" ]; then
    echo
    echo "(Running inside tmux: slides are repainted with 'tmux refresh-client',"
    echo " so there should be NO flash of the underlying pane between slides."
    echo " A diagnostics line should have printed on exit.)"
    ask_yn "Did every slide render fully -- no clipped or partial images?" || true
    ask_yn "Did slides change without flashing the pane underneath?" || true
    ask_yn "Did the exit diagnostics report 0 'never arrived'?" || true
fi

echo
echo -e "${BOLD}Results:${RESET} ${GREEN}${PASS} passed${RESET}, ${RED}${FAIL} failed${RESET}, ${YELLOW}${SKIP} skipped${RESET}"
[ "$FAIL" -eq 0 ]
