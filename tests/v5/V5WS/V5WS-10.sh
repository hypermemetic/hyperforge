#!/usr/bin/env bash
# tier: 1
# V5WS checkpoint: aggregate sibling scripts. No new behavior.
set -uo pipefail

here="$(dirname "$0")"

declare -A STORY_TO_SCRIPTS=(
  [U1]="V5WS-4.sh V5WS-2.sh V5WS-3.sh"
  [U2]="V5WS-8.sh"
  [U3]="V5WS-8.sh"
  [U4]="V5WS-7.sh V5WS-5.sh"
  [U5]="V5WS-9.sh"
  [U6]="V5WS-6.sh V5WS-3.sh V5WS-7.sh"
)

declare -A STORY_LABEL=(
  [U1]="stand up a workspace (create + list + get round-trip)"
  [U2]="rename local dir and reconcile detects via remote URL match"
  [U3]="remove local dir and reconcile drops entry from workspace yaml"
  [U4]="workspaces.remove_repo and .delete with optional forge cascade"
  [U5]="workspaces.sync aggregates per-member SyncDiff into WorkspaceSyncReport"
  [U6]="cross-org workspace: membership unit is <org>/<repo>, not the org"
)

# Tier-2 scripts: gated behind HF_TIER2=1
is_tier2_script () {
  case "$1" in
    V5WS-9.sh) return 0 ;;
    *) return 1 ;;
  esac
}

tier2_enabled () {
  [[ "${HF_TIER2:-0}" == "1" ]]
}

overall_rc=0
declare -A STORY_STATUS
declare -A STORY_DETAIL

for story in U1 U2 U3 U4 U5 U6; do
  scripts="${STORY_TO_SCRIPTS[$story]}"
  story_rc=0
  skipped_scripts=""
  failed_scripts=""
  for s in $scripts; do
    path="$here/$s"
    if [[ ! -x "$path" ]]; then
      story_rc=2
      failed_scripts="$failed_scripts $s(missing)"
      continue
    fi
    if is_tier2_script "$s" && ! tier2_enabled; then
      # Skipped under default tier-1 policy.
      if [[ $story_rc -eq 0 ]]; then
        story_rc=3
      fi
      skipped_scripts="$skipped_scripts $s"
      continue
    fi
    if ! bash "$path" >/dev/null 2>&1; then
      story_rc=1
      failed_scripts="$failed_scripts $s"
    fi
  done
  case $story_rc in
    0)
      STORY_STATUS[$story]="green"
      STORY_DETAIL[$story]="${STORY_LABEL[$story]}"
      ;;
    3)
      STORY_STATUS[$story]="yellow"
      STORY_DETAIL[$story]="${STORY_LABEL[$story]} — tier-2 gated (set HF_TIER2=1):${skipped_scripts}"
      ;;
    1)
      STORY_STATUS[$story]="red"
      STORY_DETAIL[$story]="${STORY_LABEL[$story]} — failed:${failed_scripts}"
      overall_rc=1
      ;;
    2|*)
      STORY_STATUS[$story]="red"
      STORY_DETAIL[$story]="${STORY_LABEL[$story]} — missing script(s):${failed_scripts}"
      overall_rc=1
      ;;
  esac
done

echo "=== V5WS checkpoint — state-of-epic ==="
for story in U1 U2 U3 U4 U5 U6; do
  echo "$story ${STORY_STATUS[$story]}: ${STORY_DETAIL[$story]}"
done

exit "$overall_rc"
