#!/usr/bin/env bash
# Run Poni's scripted self-test (`poni.exe --selftest <dir>`) under Wine + Xvfb.
#
# Fallback for when GitHub Actions' Windows runners are unavailable: the real
# Win32 / Direct2D / DirectWrite code paths run (as implemented by Wine) on a
# virtual 1920x1080 screen (PONI_SELFTEST_SCREEN), every state is rendered and checked, and the
# screenshots plus report.json land in <out-dir>.
#
# Usage:   tools/run-selftest-wine.sh [poni.exe] [out-dir]
#          poni.exe defaults to the cross build (cargo wbuild):
#            target/x86_64-pc-windows-gnu/release/poni.exe
#          out-dir defaults to selftest-out
# Needs:   wine (wine64), xvfb-run (Debian/Ubuntu: apt install wine64 xvfb)
# Env:     PONI_SELFTEST_TIMEOUT  seconds before the run is killed (default 180)
#          PONI_SELFTEST_SCREEN   Xvfb's screen (default 1920x1080x24; a 400 % run
#                                 needs a bigger one, e.g. 4096x2400x24)
#
# Exit code: 0 when the self-test passes, 1 when a check fails or the run
# breaks, 2 for a usage/setup problem. Wine's rendering is close to Windows but
# not identical, so a Wine failure is a hint; Windows CI is the reference.
set -euo pipefail

EXE=${1:-target/x86_64-pc-windows-gnu/release/poni.exe}
OUT=${2:-selftest-out}
TIMEOUT_S=${PONI_SELFTEST_TIMEOUT:-180}
SCREEN=${PONI_SELFTEST_SCREEN:-1920x1080x24}

die() {
    echo "error: $*" >&2
    exit 2
}

[ -f "$EXE" ] || die "$EXE not found (build it with: cargo wbuild)"
WINE=$(command -v wine64 || command -v wine || true)
[ -n "$WINE" ] || die "wine64/wine not found (apt install wine64)"
command -v xvfb-run >/dev/null 2>&1 || die "xvfb-run not found (apt install xvfb)"
command -v timeout >/dev/null 2>&1 || die "timeout not found (coreutils)"

EXE_ABS="$(cd "$(dirname "$EXE")" && pwd)/$(basename "$EXE")"
mkdir -p "$OUT"
OUT_ABS="$(cd "$OUT" && pwd)"

# A fresh, throw-away prefix for every run.
PREFIX="$(mktemp -d /tmp/poni-wine.XXXXXX)"
cleanup() {
    WINEPREFIX="$PREFIX" "$WINE" wineboot --kill >/dev/null 2>&1 || true
    if command -v wineserver >/dev/null 2>&1; then
        WINEPREFIX="$PREFIX" wineserver -k >/dev/null 2>&1 || true
    fi
    rm -rf "$PREFIX"
}
trap cleanup EXIT

export WINE EXE_ABS OUT_ABS TIMEOUT_S
export WINEPREFIX="$PREFIX" WINEARCH=win64 WINEDEBUG=-all
# No Mono / Gecko install prompts in a headless prefix.
export WINEDLLOVERRIDES="mscoree,mshtml="

echo "== Poni self-test under $("$WINE" --version 2>/dev/null || echo wine)"
echo "   exe:    $EXE_ABS"
echo "   out:    $OUT_ABS"
echo "   prefix: $PREFIX"

set +e
xvfb-run -a -s "-screen 0 $SCREEN" bash -c '
    "$WINE" wineboot --init >/dev/null 2>&1
    if command -v wineserver >/dev/null 2>&1; then wineserver -w; fi
    win_out="$("$WINE" winepath -w "$OUT_ABS" 2>/dev/null | tr -d "\r")"
    [ -n "$win_out" ] || win_out="$OUT_ABS"
    echo "   running: poni.exe --selftest $win_out"
    timeout "$TIMEOUT_S" "$WINE" "$EXE_ABS" --selftest "$win_out"
'
code=$?
set -e

echo
echo "== exit code: $code"
[ "$code" -eq 124 ] && echo "   (killed after ${TIMEOUT_S}s)"

REPORT="$OUT_ABS/report.json"
report_ok=0
if [ -f "$REPORT" ]; then
    echo "== report.json"
    if command -v python3 >/dev/null 2>&1; then
        python3 -m json.tool "$REPORT" || cat "$REPORT"
        python3 -c 'import json, sys; sys.exit(0 if json.load(open(sys.argv[1])).get("ok") is True else 1)' "$REPORT" && report_ok=1
    else
        cat "$REPORT"
        grep -qE '^  "ok": true,' "$REPORT" && report_ok=1
    fi
else
    echo "== report.json is missing"
fi

echo
echo "== screenshots"
shopt -s nullglob
pngs=("$OUT_ABS"/*.png)
for f in "${pngs[@]}"; do
    echo "   $(basename "$f")"
done
echo "   (${#pngs[@]} files in $OUT_ABS)"

if [ "$code" -eq 0 ] && [ "$report_ok" -eq 1 ]; then
    echo
    echo "== self-test passed"
    exit 0
fi

LOG="$OUT_ABS/home/cache/poni.log"
if [ -f "$LOG" ]; then
    echo
    echo "== last lines of poni.log"
    tail -n 60 "$LOG"
fi
echo
echo "== self-test FAILED"
exit 1
