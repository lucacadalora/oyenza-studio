#!/bin/bash
# Throwaway (qa branch only): open a DMG the way a person does and screenshot Finder's window.
#   qa/dmg-shot.sh <dmg> <name> <light|dark>
set -uo pipefail
dmg="$1"; name="$2"; mode="$3"
t() { perl -e 'alarm shift; exec @ARGV' "$@"; }
if [ "$mode" = dark ]; then
  t 20 osascript -e 'tell application "System Events" to tell appearance preferences to set dark mode to true' \
    || { defaults write -g AppleInterfaceStyle Dark; killall Finder; }
else
  t 20 osascript -e 'tell application "System Events" to tell appearance preferences to set dark mode to false' \
    || { defaults delete -g AppleInterfaceStyle; killall Finder; }
fi
echo "appearance: $(defaults read -g AppleInterfaceStyle 2> /dev/null || echo Light)"
sleep 3
out="$(hdiutil attach "$dmg" -readonly)"
echo "$out"
vol="$(printf '%s\n' "$out" | awk -F'\t' '/\/Volumes\// {print $NF}' | tail -1)"
ls -la "$vol"
open "$vol"
sleep 5
t 20 osascript -e 'tell application "Finder" to get bounds of front window' || echo "(no bounds)"
mkdir -p shots
screencapture -x "shots/$name-$mode.png"
t 20 osascript -e 'tell application "Finder" to close every window' || true
sleep 1
hdiutil detach "$vol" -force
sleep 2
