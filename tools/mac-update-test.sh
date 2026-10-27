#!/bin/bash
# A Mac copy installs its own update (docs/updates.md), on a real Mac: an installed copy finds a newer version
# waiting with Install on next start, takes the app out of its DMG, checks it, swaps it into its own place, quits,
# and the new version opens.
#
#   tools/mac-update-test.sh <built "Oyenza Studio.app">
#
# The newer version is this build with a higher version in its Info.plist (signed ad hoc again), in a DMG made
# the way releases make theirs (tools/make_mac_dmg.sh). The installed copy is this build as it is, in a temporary
# Applications folder. Its data folder (PONI_HOME) holds what a look at the update file, a download and Install
# on next start would have left: update.json naming that version, and the DMG in cache/update. Nothing goes
# online. CI's Macs can't show what macOS's App Management does on a person's Mac (nothing there was opened
# through Gatekeeper); that takes one try on a real one.
#
# macOS's own bash is 3.2: no empty arrays under `set -u`.
set -euo pipefail

app="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
work="${RUNNER_TEMP:-$(mktemp -d)}/poni-update-test"
next=99.0.0
rm -rf "$work"
mkdir -p "$work/Applications" "$work/home/cache/update" "$work/new"
installed="$work/Applications/Oyenza Studio.app"
running="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"

# The newer version: this build, numbered $next, sealed again.
ditto "$app" "$work/new/Oyenza Studio.app"
for key in CFBundleShortVersionString CFBundleVersion; do
  plutil -replace "$key" -string "$next" "$work/new/Oyenza Studio.app/Contents/Info.plist"
done
codesign --force --deep --sign - "$work/new/Oyenza Studio.app"
dmg="$work/home/cache/update/Oyenza-Studio-$next-mac.dmg"
bash "$root/tools/make_mac_dmg.sh" "$work/new/Oyenza Studio.app" "$dmg" test
size="$(stat -f %z "$dmg")"
sha="$(shasum -a 256 "$dmg" | cut -d ' ' -f 1)"

# What a look, the download and Install on next start leave behind.
release="https://github.com/lucacadalora/oyenza-studio/releases"
cat > "$work/home/update.json" << JSON
{
  "checked": $(date +%s),
  "latest": {
    "version": "$next",
    "date": "2026-10-27",
    "headline": "The update test",
    "notes": "$release/tag/v$next",
    "windows": null,
    "mac": { "url": "$release/download/v$next/Oyenza-Studio-$next-mac.dmg", "size": $size, "sha256": "$sha" }
  },
  "announced": "$next",
  "ran": "$running",
  "install_on_start": "$next"
}
JSON

# The installed copy starts, installs and quits.
ditto "$app" "$installed"
fail() {
  echo "::error title=Mac update test::$1"
  echo "--- poni.log"; cat "$work/home/poni.log" 2> /dev/null || true
  echo "--- stdout"; cat "$work/stdout.txt" 2> /dev/null || true
  pkill -f "$installed/Contents/MacOS/poni" 2> /dev/null || true
  exit 1
}
PONI_HOME="$work/home" "$installed/Contents/MacOS/poni" > "$work/stdout.txt" 2>&1 &
pid=$!
for _ in $(seq 1 600); do kill -0 "$pid" 2> /dev/null || break; sleep 0.1; done
kill -0 "$pid" 2> /dev/null && fail "the installed copy didn't install and quit within 60 s"
code=0
wait "$pid" || code=$?
[ "$code" = 0 ] || fail "the installed copy exited with $code"

# The new version is where the old copy was, still sealed, and Install on next start is used up.
got="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$installed/Contents/Info.plist")"
[ "$got" = "$next" ] || fail "Applications holds $got, not $next"
codesign --verify --deep --strict "$installed" || fail "the installed app's signature doesn't check out"
grep -q '"install_on_start": ""' "$work/home/update.json" || fail "update.json still asks to install on next start"

# Once the old copy has quit, the little shell deletes it and opens the new one.
for _ in $(seq 1 150); do [ -e "$work/Applications/.Oyenza Studio update.app" ] || break; sleep 0.2; done
[ -e "$work/Applications/.Oyenza Studio update.app" ] && fail "the old copy is still next to the new one"
opened=""
for _ in $(seq 1 150); do
  opened="$(pgrep -f "$installed/Contents/MacOS/poni" || true)"
  [ -n "$opened" ] && break
  sleep 0.2
done
[ -n "$opened" ] || fail "the new version didn't open"
ls -la "$work/Applications"
echo "--- poni.log"; cat "$work/home/poni.log"
pkill -f "$installed/Contents/MacOS/poni" || true
echo "The installed $running copy put $next in its own place and opened it."
