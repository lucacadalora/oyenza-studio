#!/bin/bash
# Put the Mac app in a DMG that opens as an installer window: the app on the left, an arrow, a link to
# Applications on the right, "Drag Oyenza Studio into Applications." above them and Oyen below, with the app's
# icon as the disk's icon. Runs on a Mac (hdiutil, ditto, tiffutil, SetFile from Xcode's command line tools).
#
#   tools/make_mac_dmg.sh <app> <out .dmg> <test | signed>
#
# "test" is the ad-hoc DMG tools/make_mac_app.sh builds, whose window also says how to get past Open Anyway;
# "signed" is the one tools/sign_mac_dmg.sh builds for Apple to notarize. The window's layout is
# tools/mac_dmg_settings.py, its picture installer/mac/dmg/ (tools/make_dmg_art.py draws it). dmgbuild writes the
# window's settings straight into the DMG, without Finder or AppleScript, so it works on CI's headless Macs. It
# installs into a throwaway venv unless DMGBUILD names a dmgbuild that is already there; it needs Python 3.10 or
# newer, which a Mac's /usr/bin/python3 isn't (Homebrew's is).
#
# macOS's own bash is 3.2: no empty arrays under `set -u`, hence the ${var:+...} below.
set -euo pipefail

app="$1"; dmg="$2"; variant="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
case "$variant" in
  test) background="$root/installer/mac/dmg/background-test.png" ;;
  signed) background="$root/installer/mac/dmg/background.png" ;;
  *) echo "variant must be test or signed, not $variant" >&2; exit 1 ;;
esac
[ -d "$app" ] || { echo "no app at $app" >&2; exit 1; }

plist="$app/Contents/Info.plist"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
icon_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$plist" 2> /dev/null || true)"
icon="$app/Contents/Resources/${icon_name%.icns}.icns"
[ -n "$icon_name" ] && [ -f "$icon" ] || icon=""
volume="$(basename "$app" .app) $version"

work="$(mktemp -d)"
trap 'hdiutil detach "$work/mnt" -quiet 2> /dev/null || true; rm -rf "$work"' EXIT
dmgbuild="${DMGBUILD:-}"
if [ -z "$dmgbuild" ]; then
  python3 -m venv "$work/venv"
  "$work/venv/bin/pip" install --quiet --disable-pip-version-check dmgbuild==1.6.7 ds_store==1.3.3 mac_alias==2.2.3
  dmgbuild="$work/venv/bin/dmgbuild"
fi

mkdir -p "$(dirname "$dmg")"
rm -f "$dmg"
"$dmgbuild" -s "$root/tools/mac_dmg_settings.py" -D app="$app" -D background="$background" ${icon:+-D icon="$icon"} \
  "$volume" "$dmg"

# The copy in the DMG must still be the app that was signed (dmgbuild copies with ditto, which keeps the seal).
mkdir -p "$work/mnt"
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$work/mnt" -quiet
ls -la "$work/mnt"
codesign --verify --strict "$work/mnt/$(basename "$app")"
hdiutil detach "$work/mnt" -quiet
echo "$variant DMG \"$volume\": $(stat -f %z "$dmg") bytes"
