#!/usr/bin/env bash
# tier: 1
# V5REPOS checkpoint: aggregate sibling scripts. No new behavior.
set -uo pipefail

here="$(dirname "$0")"

declare -A STORY_TO_SCRIPTS=(
  [U1]="V5REPOS-5.sh V5REPOS-4.sh V5REPOS-12.sh"
  [U2]="V5REPOS-7.sh"
  [U3]="V5REPOS-6.sh"
  [U4]="V5REPOS-13.sh"
  [U5]="V5REPOS-14.sh"
  [U6]="V5REPOS-13.sh V5REPOS-9.sh V5REPOS-10.sh"
  [U7]="V5REPOS-12.sh V5REPOS-4.sh"
)

declare -A STORY_LABEL=(
  [U1]="register: add a repo by name+URL, provider auto-resolves"
  [U2]="add mirror: second remote, no creds needed at add time"
  [U3]="remove without destroying: default local-only"
  [U4]="sync: drift between local and forge reported, no writes"
  [U5]="push: local metadata applied to forge per D4"
  [U6]="cross-provider: GitHub + Codeberg dispatch per-remote"
  [U7]="custom-domain provider: override / provider_map works"
)

# Also sanity-check the trait ticket separately.
TRAIT_SCRIPT="V5REPOS-2.sh"
CRUD_SCRIPTS="V5REPOS-3.sh V5REPOS-8.sh"

overall_rc=0
declare -A STORY_STATUS
declare -A STORY_DETAIL

run_script() {
  local path="$1"
  if [[ ! -x "$path" ]]; then
    echo "missing"
    return 2
  fi
  local out rc
  set +e
  out=$(bash "$path" 2>&1)
  rc=$?
  set -e
  # SKIP (stdout contains 'SKIP:' on a line) counts as yellow not red.
  if [[ $rc -eq 0 && "$out" == *"SKIP:"* ]]; then
    echo "skip"
    return 0
  fi
  if [[ $rc -eq 0 ]]; then
    echo "ok"
    return 0
  fi
  echo "fail"
  return 1
}

for story in U1 U2 U3 U4 U5 U6 U7; do
  scripts="${STORY_TO_SCRIPTS[$story]}"
  story_state="green"
  detail_tokens=""
  for s in $scripts; do
    result=$(run_script "$here/$s")
    case "$result" in
      ok) ;;
      skip) [[ "$story_state" == "green" ]] && story_state="yellow"; detail_tokens="$detail_tokens $s(skip)" ;;
      missing) story_state="red"; detail_tokens="$detail_tokens $s(missing)"; overall_rc=1 ;;
      fail|*) story_state="red"; detail_tokens="$detail_tokens $s(fail)"; overall_rc=1 ;;
    esac
  done
  STORY_STATUS[$story]="$story_state"
  STORY_DETAIL[$story]="${STORY_LABEL[$story]}${detail_tokens:+ —$detail_tokens}"
done

# Trait and remaining CRUD scripts: if they fail, the epic is red overall.
for s in $TRAIT_SCRIPT $CRUD_SCRIPTS; do
  result=$(run_script "$here/$s")
  case "$result" in
    ok|skip) ;;
    *) overall_rc=1; echo "CRITICAL: $s $result" ;;
  esac
done

echo "=== V5REPOS checkpoint — state-of-epic ==="
for story in U1 U2 U3 U4 U5 U6 U7; do
  echo "$story ${STORY_STATUS[$story]}: ${STORY_DETAIL[$story]}"
done

exit "$overall_rc"
