#!/usr/bin/env bash
# tier: 1
# V5ORGS checkpoint: aggregate sibling scripts. No new behavior.
set -uo pipefail

here="$(dirname "$0")"

declare -A STORY_TO_SCRIPTS=(
  [U1]="V5ORGS-4.sh V5ORGS-7.sh"
  [U2]="V5ORGS-3.sh"
  [U3]="V5ORGS-7.sh"
  [U4]="V5ORGS-5.sh"
  [U5]="V5ORGS-2.sh V5ORGS-4.sh V5ORGS-8.sh"
  [U6]="V5ORGS-6.sh"
  [U7]="V5ORGS-8.sh"
)

declare -A STORY_LABEL=(
  [U1]="onboard a new org (create + set_credential from scratch)"
  [U2]="inspect without leaking (orgs.get redaction)"
  [U3]="rotate a credential (set_credential replaces same-key entry in place)"
  [U4]="delete with confidence (dry_run preview, siblings intact)"
  [U5]="survive restart (list/get on fresh daemon matches pre-restart state)"
  [U6]="patch provider without clobbering credentials or repos"
  [U7]="remove one credential without touching others or the secret store"
)

overall_rc=0
declare -A STORY_STATUS
declare -A STORY_DETAIL

for story in U1 U2 U3 U4 U5 U6 U7; do
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

echo "=== V5ORGS checkpoint — state-of-epic ==="
for story in U1 U2 U3 U4 U5 U6 U7; do
  echo "$story ${STORY_STATUS[$story]}: ${STORY_DETAIL[$story]}"
done

exit "$overall_rc"
