#!/bin/sh
# Ships Cua Driver inside a built Bricks.app (Contents/Resources/CuaDriver.app),
# keeping the driver's own signature: macOS then attributes its Accessibility and
# Screen Recording grants to the driver (com.trycua.driver), whichever copy runs.
#
#   scripts/embed-cua-driver.sh <Bricks.app> [CuaDriver.app]
#
# The driver defaults to /Applications/CuaDriver.app.
set -eu

bricks=${1:?usage: embed-cua-driver.sh <Bricks.app> [CuaDriver.app]}
driver=${2:-/Applications/CuaDriver.app}

[ -d "$bricks/Contents" ] || { echo "not an app bundle: $bricks" >&2; exit 1; }
[ -x "$driver/Contents/MacOS/cua-driver" ] || { echo "no Cua Driver at $driver" >&2; exit 1; }
codesign --verify --strict "$driver" || { echo "Cua Driver's signature does not verify" >&2; exit 1; }

target="$bricks/Contents/Resources/CuaDriver.app"
rm -rf "$target"
ditto "$driver" "$target"
codesign --verify --strict "$target"
codesign -dv "$target" 2>&1 | grep -E '^(Identifier|TeamIdentifier)='
