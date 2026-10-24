#!/bin/bash
# The Mac smoke run (mac.yml): start the app with --smoke and answer it. At
# each state the app writes where its window is (smoke/ready-<n>-<name>.txt)
# and waits; this script takes the screenshot, does what CI does for that
# step (a real copy, a Spotify notification) and answers. Prints the report
# and the log's tail, and exits with the app's code.
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
      *clipboard-copy) printf 'Oyen Poni smoke test, copied at %s' "$(date +%T)" | pbcopy; sleep 1.5 ;;
      # What Spotify announces when a track plays (no Spotify on the runner).
      *music-notify)
        osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSDistributedNotificationCenter.defaultCenter.postNotificationNameObjectUserInfoDeliverImmediately("com.spotify.client.PlaybackStateChanged", $("com.spotify.client"), $({"Name": "Blinding Lights", "Artist": "The Weeknd", "Album": "After Hours", "Duration": 200040, "Playback Position": 42.5, "Player State": "Playing"}), true)' || true
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
echo "smoke exit code: $code ($shots screenshots)"
echo "---- report.json"; cat smoke/report.json || echo "(missing)"
echo "---- poni.log"; tail -80 smoke/home/poni.log || true
exit $code
