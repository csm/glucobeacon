#!/usr/bin/env bash
# Regenerate the wiring diagram in every format.
#
#   ./tools/wiring/render.sh
#
# Produces docs/wiring/display-node.{svg,pdf,png,txt}. The SVG and the text are
# generated from the firmware's board.rs; the PDF and PNG are rendered from the
# SVG by headless Chromium, which is the one renderer we can rely on being
# present (Playwright installs it).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$REPO/docs/wiring"
NAME=display-node

python3 "$REPO/tools/wiring/generate.py"

CHROME="${CHROME:-}"
if [[ -z "$CHROME" ]]; then
  for candidate in \
    /opt/pw-browsers/chromium-*/chrome-linux/chrome \
    "$(command -v chromium || true)" \
    "$(command -v chromium-browser || true)" \
    "$(command -v google-chrome || true)"; do
    if [[ -x "$candidate" ]]; then CHROME="$candidate"; break; fi
  done
fi

if [[ -z "$CHROME" ]]; then
  echo "no chromium found; SVG and text are written, skipping PDF and PNG" >&2
  exit 0
fi

# Chromium prints the page, not the file, so the SVG goes in a page sized to it
# with no margin. Otherwise the diagram lands on A4 with a wide white border.
#
# SLACK is for the screenshot only. Headless Chromium silently clips the bottom
# of the *viewport* when its height is exactly the content height — around a
# hundred pixels just vanish — so the PNG is rendered taller and trimmed back.
# Printing does not have that problem (verified: same text-operator count at
# both page heights), so @page stays at the exact size and the PDF has no
# stray white strip.
WIDTH=$(grep -o 'width="[0-9]*"' "$OUT/$NAME.svg" | head -1 | tr -dc '0-9')
HEIGHT=$(grep -o 'height="[0-9]*"' "$OUT/$NAME.svg" | head -1 | tr -dc '0-9')
SLACK=240
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cp "$OUT/$NAME.svg" "$WORK/$NAME.svg"
cat > "$WORK/page.html" <<HTML
<!doctype html><meta charset="utf-8">
<style>
  @page { size: ${WIDTH}px ${HEIGHT}px; margin: 0; }
  html, body { margin: 0; padding: 0; background: #fff; }
  img { display: block; width: ${WIDTH}px; height: ${HEIGHT}px; }
</style>
<img src="$NAME.svg">
HTML

"$CHROME" --headless --disable-gpu --no-sandbox --hide-scrollbars \
  --no-pdf-header-footer --print-to-pdf="$OUT/$NAME.pdf" \
  "file://$WORK/page.html" >/dev/null 2>&1

"$CHROME" --headless --disable-gpu --no-sandbox --hide-scrollbars \
  --force-device-scale-factor=2 --window-size="$WIDTH,$((HEIGHT + SLACK))" \
  --screenshot="$OUT/$NAME.png" \
  "file://$WORK/page.html" >/dev/null 2>&1

# The screenshot is 2x and SLACK too tall; trim the blank strip off the bottom.
python3 "$REPO/tools/wiring/crop_png.py" "$OUT/$NAME.png" $((HEIGHT * 2))

echo "wrote $OUT/$NAME.pdf"
echo "wrote $OUT/$NAME.png"
