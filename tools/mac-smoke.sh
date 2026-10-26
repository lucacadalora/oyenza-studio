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
# inside it while its music and open card float below it (notch-pill-*); its pill-theme
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
# macOS's own now playing: the MediaRemote adapter's test player (mac.yml builds it) reports a track
# there from the now-playing-start step on. The app's adapter leaves that player out unless
# ADAPTER_TEST_MODE is set, and the app checks for its track only with PONI_SMOKE_NOW_PLAYING.
client="${MEDIAREMOTE_TEST_CLIENT:-}"
client_pid=""
if [ -n "$client" ] && [ -x "$client" ]; then
  export ADAPTER_TEST_MODE=1 PONI_SMOKE_NOW_PLAYING=1
else
  echo "(no MediaRemote test player: the now-playing check is skipped)"
fi
# A real browser tab in macOS's own now playing, as YouTube in Edge reaches it: the runner's Edge
# (else Chrome) plays tools/mac-browser-player.html from the now-playing-browser-start step on.
# The app checks for its track only with PONI_SMOKE_BROWSER.
browser=""
for b in "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"; do
  if [ -x "$b" ]; then browser="$b"; break; fi
done
browser_pid=""
if [ -n "$browser" ]; then
  export PONI_SMOKE_BROWSER=1
  echo "browser for the now-playing-browser step: $("$browser" --version 2>/dev/null || basename "$browser")"
else
  echo "(no Edge or Chrome: the browser check is skipped)"
fi
# Oyenza Studio Recorder's side of the island's message port: a message sent
# from another process the way the recorder sends it (CoreFoundation's
# CFMessagePortSendRequest, through Python's ctypes). The island's reply 1
# means it took it; anything else fails the run.
recorder_failed=""
recorder_say() {
  local said
  said=$(python3 - "$1" 2>&1 <<'PY'
import ctypes, ctypes.util, sys
cf = ctypes.CDLL(ctypes.util.find_library("CoreFoundation"))
vp = ctypes.c_void_p
cf.CFStringCreateWithCString.restype = vp
cf.CFStringCreateWithCString.argtypes = [vp, ctypes.c_char_p, ctypes.c_uint32]
cf.CFMessagePortCreateRemote.restype = vp
cf.CFMessagePortCreateRemote.argtypes = [vp, vp]
cf.CFDataCreate.restype = vp
cf.CFDataCreate.argtypes = [vp, ctypes.c_char_p, ctypes.c_long]
cf.CFMessagePortSendRequest.restype = ctypes.c_int32
cf.CFMessagePortSendRequest.argtypes = [vp, ctypes.c_int32, vp, ctypes.c_double, ctypes.c_double, vp, ctypes.POINTER(vp)]
cf.CFDataGetLength.restype = ctypes.c_long
cf.CFDataGetLength.argtypes = [vp]
cf.CFDataGetBytePtr.restype = vp
cf.CFDataGetBytePtr.argtypes = [vp]
utf8 = 0x08000100
port = cf.CFMessagePortCreateRemote(None, cf.CFStringCreateWithCString(None, b"com.oyenza.poni.recorder", utf8))
if not port:
    sys.exit("no island port")
body = sys.argv[1].encode()
reply = vp()
status = cf.CFMessagePortSendRequest(port, 0x4F59524D, cf.CFDataCreate(None, body, len(body)), 1.0, 1.0,
                                     cf.CFStringCreateWithCString(None, b"com.oyenza.recorder.reply", utf8), ctypes.byref(reply))
text = ctypes.string_at(cf.CFDataGetBytePtr(reply), cf.CFDataGetLength(reply)).decode() if status == 0 and reply.value else ""
print(f"status {status} reply {text!r}")
PY
)
  echo "recorder -> island: $1: $said"
  case "$said" in *"reply '1'"*) ;; *) recorder_failed=1 ;; esac
  sleep 1
}
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
      # What Spotify announces when a track plays (no Spotify on the runner).
      *music-notify)
        osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSDistributedNotificationCenter.defaultCenter.postNotificationNameObjectUserInfoDeliverImmediately("com.spotify.client.PlaybackStateChanged", $("com.spotify.client"), $({"Name": "Blinding Lights", "Artist": "The Weeknd", "Album": "After Hours", "Duration": 200040, "Playback Position": 42.5, "Player State": "Playing"}), true)' || true
        sleep 1.5 ;;
      # Then a made-up song with Japanese lyrics (written for the test; the
      # app put them in its lyrics cache), for the Japanese readings add-on.
      *ja-notify)
        osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSDistributedNotificationCenter.defaultCenter.postNotificationNameObjectUserInfoDeliverImmediately("com.spotify.client.PlaybackStateChanged", $("com.spotify.client"), $({"Name": "Paper Boats", "Artist": "The Lanterns", "Album": "Harbor Lights", "Duration": 201000, "Playback Position": 61.0, "Player State": "Playing"}), true)' || true
        sleep 1.5 ;;
      # A player that only macOS's own now playing knows (as a browser tab is).
      *now-playing-start)
        if [ -n "${PONI_SMOKE_NOW_PLAYING:-}" ]; then
          "$client" > smoke/test-player.txt 2>&1 &
          client_pid=$!
          echo "the MediaRemote adapter's test player: pid $client_pid"
          sleep 1
        fi ;;
      # A browser tab instead: the test player stops, the browser plays the page (autoplay
      # allowed, a profile of its own with no first-run screens or keychain prompt).
      *now-playing-browser-start)
        if [ -n "$browser" ]; then
          if [ -n "$client_pid" ]; then kill "$client_pid" 2>/dev/null; client_pid=""; fi
          "$browser" --user-data-dir="${RUNNER_TEMP:-/tmp}/poni-smoke-browser" --no-first-run --no-default-browser-check \
            --use-mock-keychain --password-store=basic --autoplay-policy=no-user-gesture-required \
            "file://$(pwd)/tools/mac-browser-player.html" > smoke/browser.txt 2>&1 &
          browser_pid=$!
          echo "the browser: pid $browser_pid"
          sleep 1
        fi ;;
      # A recording, then its end, as Oyenza Studio Recorder tells the island.
      *record-port) recorder_say '{"v":1,"event":"state","state":"recording","elapsed_ms":754000}' ;;
      *record-stop) recorder_say '{"v":1,"event":"state","state":"idle","elapsed_ms":0}' ;;
    esac
    left=$(( x > 40 ? x - 40 : 0 ))
    case "$base" in
      # The menu opens under the menu bar item at the top right.
      *menu) screencapture -x "smoke/shot-$base.png" || true ;;
      *) screencapture -x -R "$left,0,$(( w + 80 )),$(( y + h + 40 ))" "smoke/shot-$base.png" || true ;;
    esac
    if [ $shots -eq 0 ]; then screencapture -x smoke/screen-full.png || true; fi
    shots=$(( shots + 1 ))
    # The browser's step, pictured with the whole screen (the island over the
    # playing tab); then the browser closes before the menu.
    case "$base" in
      *now-playing-browser)
        screencapture -x smoke/screen-browser.png || true
        if [ -n "$browser_pid" ]; then kill "$browser_pid" 2>/dev/null; browser_pid=""; fi ;;
    esac
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
if [ -n "$client_pid" ]; then kill "$client_pid" 2>/dev/null || true; fi
if [ -n "$browser_pid" ]; then kill "$browser_pid" 2>/dev/null || true; fi
if [ -n "$recorder_failed" ]; then
  echo "the island didn't take a message from the recorder's side of its port"
  code=1
fi
echo "smoke exit code: $code ($shots screenshots)"
echo "---- report.json"; cat smoke/report.json || echo "(missing)"
echo "---- poni.log"; tail -80 smoke/home/poni.log || true
exit $code
