#!/usr/bin/env bash
# Weight check, download size (CI): the installer people download, against
# "download_mb" in tools/weight-budget.json (AGENTS.md, Keep it light). Over
# budget is a warning until "warn_only_until", then it fails the run.
#   tools/ci-weight-download.sh dist/Oyen-Poni-Setup-<version>.exe
set -euo pipefail
file="$1"
budget_file="$(dirname "$0")/weight-budget.json"
mb=$(awk -v s="$(stat -c %s "$file")" 'BEGIN { printf "%.2f", s / 1e6 }')
budget=$(jq -r .download_mb "$budget_file")
until=$(jq -r .warn_only_until "$budget_file")
ok=$(awk -v a="$mb" -v b="$budget" 'BEGIN { print (a <= b) ? "ok" : "**over**" }')
{
  echo "### Weight check, download size"
  echo
  echo "| | Measured | Budget | |"
  echo "|---|---|---|---|"
  echo "| Installer ($(basename "$file")) | $mb MB | $budget MB | $ok |"
} | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
if [ "$ok" != ok ]; then
  if [[ "$(date -u +%F)" < "$until" ]]; then
    echo "::warning title=Weight check::the download, $mb MB, is over its budget of $budget MB (warnings only until $until)"
  else
    echo "::error title=Weight check::the download, $mb MB, is over its budget of $budget MB"
    exit 1
  fi
fi
