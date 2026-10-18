#!/bin/sh
# Builds the equivalence page and dumps its DOM in Chromium headless shell (real DOM, not happy-dom).
# Run from apps/bricks-desktop: sh tests/bench/browser/run.sh  -> tests/bench/results/browser/dom.html
set -e
CH="${CHROME_HEADLESS:-$HOME/Library/Caches/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-mac-arm64/chrome-headless-shell}"
npx vite build --config tests/bench/browser/vite.config.ts
cp tests/bench/browser/page.html tests/bench/results/browser/page.html
timeout 1500 "$CH" --headless --no-sandbox --disable-gpu --allow-file-access-from-files \
  --dump-dom "file://$(pwd)/tests/bench/results/browser/page.html" > tests/bench/results/browser/dom.html 2>/dev/null
echo "dom: $(wc -c < tests/bench/results/browser/dom.html) bytes"
