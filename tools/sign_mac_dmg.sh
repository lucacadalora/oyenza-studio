#!/bin/bash
# Sign the Mac app with a Developer ID, put it in a signed DMG, have Apple notarize it and staple the ticket,
# so the DMG opens with a plain "Open" instead of "Apple could not verify ... is free of malware". Runs on a Mac
# (codesign, hdiutil, xcrun).
#
#   tools/sign_mac_dmg.sh <ad-hoc .dmg> <out dir> <signing identity>
#
# The input is the test DMG tools/make_mac_app.sh builds (<name>-<version>-mac-unsigned.dmg). Its app is re-signed
# with the hardened runtime, a secure timestamp and installer/mac/poni.entitlements, and goes into a new DMG with
# the same name minus "-unsigned" and the same installer window (tools/make_mac_dmg.sh), without the test build's
# note on Open Anyway. The app is found as the DMG's only .app, so a renamed app needs no change here.
#
# <signing identity> is what `codesign --sign` takes; CI passes the certificate's SHA-1 from its keychain, and
# SIGN_KEYCHAIN names that keychain. Notarizing needs NOTARY_KEY (the App Store Connect key's .p8 file),
# NOTARY_KEY_ID and NOTARY_ISSUER. NOTARIZE=0 stops after signing: mac.yml's sign-check job signs that way with
# a throwaway certificate on every pull request, so this path is tested before a release needs it.
# docs/mac-signing.md explains the certificate, the key and the secrets.
#
# macOS's own bash is 3.2: no empty arrays under `set -u`, hence the ${var:+...} below.
set -euo pipefail

src="$1"; out="$2"; identity="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
entitlements="$root/installer/mac/poni.entitlements"

work="$(mktemp -d)"
trap 'hdiutil detach "$work/mnt" -quiet 2> /dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$work/mnt" "$work/stage" "$out"

# The app from the ad-hoc DMG.
hdiutil attach "$src" -nobrowse -readonly -mountpoint "$work/mnt" -quiet
apps=("$work/mnt"/*.app)
if [ ${#apps[@]} -ne 1 ] || [ ! -d "${apps[0]}" ]; then
  echo "expected one .app in $src" >&2
  exit 1
fi
name="$(basename "${apps[0]}")"
cp -R "${apps[0]}" "$work/stage/"
hdiutil detach "$work/mnt" -quiet
app="$work/stage/$name"

# 1. The app: hardened runtime, secure timestamp, the entitlements. Its one nested piece of code, the MediaRemote
#    adapter's framework (which /usr/bin/perl loads, not the app), is signed first, so no --deep (Apple advises
#    against it for signing). --force replaces the ad-hoc signatures.
for framework in "$app/Contents/Frameworks/"*.framework; do
  [ -d "$framework" ] || continue
  codesign --force --options runtime --timestamp --sign "$identity" ${SIGN_KEYCHAIN:+--keychain "$SIGN_KEYCHAIN"} "$framework"
done
codesign --force --options runtime --timestamp --entitlements "$entitlements" \
  --sign "$identity" ${SIGN_KEYCHAIN:+--keychain "$SIGN_KEYCHAIN"} "$app"
codesign --verify --strict --verbose=2 "$app"
codesign --display --verbose=2 "$app" > "$work/app-signature.txt" 2>&1
cat "$work/app-signature.txt"
grep -q "flags=.*runtime" "$work/app-signature.txt" || { echo "the app isn't signed with the hardened runtime" >&2; exit 1; }
grep -q "^Timestamp=" "$work/app-signature.txt" || { echo "the app's signature has no secure timestamp" >&2; exit 1; }
codesign --display --entitlements - --xml "$app" 2> /dev/null | grep -q "com.apple.security.automation.apple-events" \
  || { echo "the app's signature lacks the Apple Events entitlement (installer/mac/poni.entitlements)" >&2; exit 1; }
codesign --display --entitlements - --xml "$app" 2> /dev/null | grep -q "com.apple.security.personal-information.location" \
  || { echo "the app's signature lacks the location entitlement (installer/mac/poni.entitlements)" >&2; exit 1; }

# 2. The DMG: the installer window with the signed app, signed too.
base="$(basename "$src" .dmg)"
dmg="$out/${base%-unsigned}.dmg"
bash "$root/tools/make_mac_dmg.sh" "$app" "$dmg" signed
codesign --force --timestamp --sign "$identity" ${SIGN_KEYCHAIN:+--keychain "$SIGN_KEYCHAIN"} "$dmg"
codesign --verify --verbose=2 "$dmg"

# 3. Apple's notary service checks the DMG and the app in it. The ticket is stapled to the DMG, so Gatekeeper
#    accepts a downloaded copy even offline.
if [ "${NOTARIZE:-1}" = 1 ]; then
  : "${NOTARY_KEY:?path of the App Store Connect .p8 key}" "${NOTARY_KEY_ID:?the key ID}" "${NOTARY_ISSUER:?the issuer ID}"
  xcrun notarytool submit "$dmg" --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" \
    --wait --timeout 45m --output-format json > "$work/notary.json" || true
  cat "$work/notary.json"; echo
  read -r id status < <(python3 -c 'import json, sys; d = json.load(open(sys.argv[1])); print(d.get("id", "-"), d.get("status", "-"))' \
    "$work/notary.json" 2> /dev/null || echo "- -")
  if [ "$status" != Accepted ]; then
    echo "Apple's notary service answered: $status" >&2
    if [ "$id" != - ]; then
      xcrun notarytool log "$id" --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" || true
    fi
    exit 1
  fi
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  # What Gatekeeper says about a downloaded copy: "accepted", source=Notarized Developer ID.
  spctl --assess --type open --context context:primary-signature --verbose=4 "$dmg"
  hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$work/mnt" -quiet
  spctl --assess --type execute --verbose=4 "$work/mnt/$name"
  hdiutil detach "$work/mnt" -quiet
else
  echo "NOTARIZE=0: signed, not notarized (Gatekeeper would still stop a downloaded copy)"
fi

echo "ad-hoc DMG $(stat -f %z "$src") bytes, signed DMG $(stat -f %z "$dmg") bytes"
shasum -a 256 "$dmg"
echo "built $dmg"
