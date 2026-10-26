#!/bin/bash
# The Mac smoke run (mac.yml): start the app with --smoke and answer it. At
# each state the app writes where its window is (smoke/ready-<n>-<name>.txt)
# and waits; this script takes the screenshot, does what CI does for that
# step (a real copy, a Spotify notification) and answers. Prints the report
# and the log's tail, and exits with the app's code.
#
# The theme steps (glass-*, pill-*, notch-*) switch the app's own theme; the
# notch-* steps pretend the screen has a MacBook's notch and the app paints
# it black where the camera would be, so the pictures show what a MacBook
# shows. Pill floats below the top edge (pill-*), and around the notch rests
# in the menu bar's line and opens below it (notch-pill-*); its pill-theme
# check in the report says where the island sat and which points took the
# mouse.
#
# Usage: tools/mac-smoke.sh "<path to the app's poni binary>"
# Not -e: the report must print even when a check fails.
set +e -u
system_profiler SPDisplaysDataType | grep -E "Resolution|Retina|Main Display" || true
bin="$1"
# A real wallpaper behind the island (the runner's desktop is plain black),
# so the pictures show what Glass does over a bright, busy desktop: the
# website's own picture, set through AppKit (no permission needed).
cat > /tmp/poni-wallpaper.js <<'JXA'
function run(argv) {
  ObjC.import("AppKit");
  var url = $.NSURL.fileURLWithPath(argv[0]);
  var screens = $.NSScreen.screens;
  for (var i = 0; i < screens.count; i++) {
    $.NSWorkspace.sharedWorkspace.setDesktopImageURLForScreenOptionsError(url, screens.objectAtIndex(i), $({}), null);
  }
  return "wallpaper: " + argv[0];
}
JXA
osascript -l JavaScript /tmp/poni-wallpaper.js "$(pwd)/site/assets/wall/morning.jpg" || echo "(could not set the wallpaper)"
# Glass and blur draw flat under Reduce Transparency; say whether it's on,
# and ask for it off (a runner image may not allow it).
echo "reduce transparency: $(defaults read com.apple.universalaccess reduceTransparency 2>&1)"
defaults write com.apple.universalaccess reduceTransparency -bool false 2>&1 || true
echo "reduce transparency now: $(defaults read com.apple.universalaccess reduceTransparency 2>&1)"
sleep 1
mkdir -p smoke
# A stand-in for a browser tab or a music app (tools/mac-fake-player.m), for the
# nowplaying-* steps: it reports a song to macOS's Now Playing.
player="$(pwd)/smoke/Smoke Player.app"
mkdir -p "$player/Contents/MacOS"
clang -fobjc-arc -O1 -mmacosx-version-min=14.0 -framework AppKit -framework MediaPlayer \
  tools/mac-fake-player.m -o "$player/Contents/MacOS/smoke-player" || echo "(could not build the stand-in player)"
cat > "$player/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>com.oyenza.studio.smoke-player</string>
	<key>CFBundleName</key><string>Smoke Player</string>
	<key>CFBundleExecutable</key><string>smoke-player</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>LSUIElement</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$player" || true
# The Now Playing adapter's processes (perl with the adapter's framework, and the
# small one that ends it with the app): what they hold, and their CPU time so far.
adapter_cost() {
  ps -axo pid=,rss=,time=,command= | grep -F "now-playing.pl" | grep -v grep | while read -r p rss t rest; do
    echo "$1: now playing adapter pid $p, resident $((rss / 1024)) MB, cpu time $t, $(footprint -p "$p" 2>/dev/null | grep -m1 -i "footprint:" | sed 's/^ *//')"
  done | tee -a smoke/nowplaying-cost.txt
}
"$bin" --smoke smoke > smoke/stdout.txt 2>&1 &
pid=$!
deadline=$((SECONDS + 300))
shots=0
while kill -0 $pid 2>/dev/null && [ $SECONDS -lt $deadline ]; do
  for r in smoke/ready-*.txt; do
    [ -e "$r" ] || continue
    base="${r#smoke/ready-}"; base="${base%.txt}"
    [ -e "smoke/shot-$base.done" ] && continue
    IFS=, read -r x y w h < "$r"
    case "$base" in
      # A real copy: the app's clipboard history should pick it up.
      *clipboard-copy) printf 'Oyenza Studio smoke test, copied at %s' "$(date +%T)" | pbcopy; sleep 1.5 ;;
      # What Spotify announces when a track plays (no Spotify on the runner).
      *music-notify)
        osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSDistributedNotificationCenter.defaultCenter.postNotificationNameObjectUserInfoDeliverImmediately("com.spotify.client.PlaybackStateChanged", $("com.spotify.client"), $({"Name": "Blinding Lights", "Artist": "The Weeknd", "Album": "After Hours", "Duration": 200040, "Playback Position": 42.5, "Player State": "Playing"}), true)' || true
        sleep 1.5 ;;
      # Then a made-up song with Japanese lyrics (written for the test; the
      # app put them in its lyrics cache), for the Japanese readings add-on.
      # Another app plays: the stand-in reports its song to macOS's Now Playing.
      *nowplaying-start)
        adapter_cost "before the song"
        rm -f smoke/home/smoke-player.txt
        open -g -n "$player" --args "$(pwd)/smoke/home/smoke-player.txt" || echo "(could not start the stand-in player)"
        sleep 1.5 ;;
      *nowplaying-check) adapter_cost "the song showing" ;;
      *nowplaying-quit) pkill -f "Smoke Player.app/Contents/MacOS/smoke-player" || true; sleep 1 ;;
      *menu) adapter_cost "at the end" ;;
      *ja-notify)
        osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSDistributedNotificationCenter.defaultCenter.postNotificationNameObjectUserInfoDeliverImmediately("com.spotify.client.PlaybackStateChanged", $("com.spotify.client"), $({"Name": "Paper Boats", "Artist": "The Lanterns", "Album": "Harbor Lights", "Duration": 201000, "Playback Position": 61.0, "Player State": "Playing"}), true)' || true
        sleep 1.5 ;;
    esac
    left=$(( x > 40 ? x - 40 : 0 ))
    case "$base" in
      # The menu opens under the menu bar item at the top right.
      *menu) screencapture -x "smoke/shot-$base.png" || true ;;
      *) screencapture -x -R "$left,0,$(( w + 80 )),$(( y + h + 40 ))" "smoke/shot-$base.png" || true ;;
    esac
    if [ $shots -eq 0 ]; then screencapture -x smoke/screen-full.png || true; fi
    shots=$(( shots + 1 ))
    touch "smoke/shot-$base.done"
  done
  sleep 0.2
done
if kill -0 $pid 2>/dev/null; then
  echo "smoke run did not finish within 300 s"
  kill $pid
  code=1
else
  wait $pid
  code=$?
fi
pkill -f "Smoke Player.app/Contents/MacOS/smoke-player" 2>/dev/null
# The adapter ends with the app (installer/mac/now-playing.pl).
sleep 1
left=$(pgrep -f "now-playing.pl" || true)
if [ -n "$left" ]; then
  echo "the Now Playing adapter outlived the app:"; ps -o pid=,command= -p $(echo $left | tr ' ' ',') || true
  pkill -f "now-playing.pl" || true
  code=1
else
  echo "the Now Playing adapter stopped with the app"
fi
echo "---- now playing"; cat smoke/nowplaying-cost.txt 2>/dev/null; cat smoke/home/now-playing.log 2>/dev/null
echo "smoke exit code: $code ($shots screenshots)"
echo "---- report.json"; cat smoke/report.json || echo "(missing)"
echo "---- poni.log"; tail -80 smoke/home/poni.log || true
exit $code
