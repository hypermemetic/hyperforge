#!/usr/bin/env bash
# tier: 1
# V5CORE checkpoint: aggregate sibling scripts. No new behavior.
set -uo pipefail

here="$(dirname "$0")"

declare -A STORY_TO_SCRIPTS=(
  [U1]="V5CORE-2.sh"
  [U2]="V5CORE-5.sh"
  [U3]="V5CORE-6.sh V5CORE-7.sh V5CORE-8.sh"
  [U4]="V5CORE-3.sh"
  [U5]="V5CORE-4.sh"
  [U6]="V5CORE-9.sh"
)

declare -A STORY_LABEL=(
  [U1]="daemon starts on 44105 without touching v4"
  [U2]="status returns version + config_dir"
  [U3]="three stubs (orgs/repos/workspaces) discoverable, zero methods each"
  [U4]="every config fixture round-trips losslessly"
  [U5]="secrets:// reference resolves through embedded store"
  [U6]="shared harness spawns, runs, tears down cleanly"
)

overall_rc=0
declare -A STORY_STATUS
declare -A STORY_DETAIL

for story in U1 U2 U3 U4 U5 U6; do
  scripts="${STORY_TO_SCRIPTS[$story]}"
  story_rc=0
  failed_scripts=""
  for s in $scripts; do
    path="$here/$s"
    if [[ ! -x "$path" ]]; then
      story_rc=2
      failed_scripts="$failed_scripts $s(missing)"
      continue
    fi
    if ! bash "$path" >/dev/null 2>&1; then
      story_rc=1
      failed_scripts="$failed_scripts $s"
    fi
  done
  if [[ $story_rc -eq 0 ]]; then
    STORY_STATUS[$story]="green"
    STORY_DETAIL[$story]="${STORY_LABEL[$story]}"
  elif [[ $story_rc -eq 1 ]]; then
    STORY_STATUS[$story]="red"
    STORY_DETAIL[$story]="${STORY_LABEL[$story]} — failed:${failed_scripts}"
    overall_rc=1
  else
    STORY_STATUS[$story]="red"
    STORY_DETAIL[$story]="${STORY_LABEL[$story]} — missing script(s):${failed_scripts}"
    overall_rc=1
  fi
done

echo "=== V5CORE checkpoint — state-of-epic ==="
for story in U1 U2 U3 U4 U5 U6; do
  echo "$story ${STORY_STATUS[$story]}: ${STORY_DETAIL[$story]}"
done

exit "$overall_rc"
