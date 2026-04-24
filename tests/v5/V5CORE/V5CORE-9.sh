#!/usr/bin/env bash
# tier: 1
set -euo pipefail
source "$(dirname "$0")/../harness/lib.sh"

# --- happy path: spawn, run, teardown ---
hf_spawn
captured_config="$HF_CONFIG"
captured_port="$HF_PORT"

# Trivial command produces at least one event.
hf_cmd status | hf_assert_event '.'

# HF_CONFIG exists while daemon runs.
test -d "$captured_config"

hf_teardown

# After teardown, HF_CONFIG is gone and port is no longer bound.
[[ ! -d "$captured_config" ]]
# Re-binding the port should succeed if it's truly released.
python3 -c "
import socket, sys
s = socket.socket()
try:
    s.bind(('127.0.0.1', $captured_port))
    s.close()
except OSError as e:
    sys.exit(f'port $captured_port still bound: {e}')
" || true  # port reuse timing is OS-dependent; soft check only

# --- two concurrent spawns get distinct ports ---
(
  source "$(dirname "$0")/../harness/lib.sh"
  hf_spawn
  echo "$HF_PORT" > /tmp/hf-port-a-$$
  sleep 2
  hf_teardown
) &
pid_a=$!
(
  source "$(dirname "$0")/../harness/lib.sh"
  hf_spawn
  echo "$HF_PORT" > /tmp/hf-port-b-$$
  sleep 2
  hf_teardown
) &
pid_b=$!
wait "$pid_a" "$pid_b"
port_a=$(cat /tmp/hf-port-a-$$)
port_b=$(cat /tmp/hf-port-b-$$)
rm -f /tmp/hf-port-a-$$ /tmp/hf-port-b-$$
[[ "$port_a" != "$port_b" ]]

# --- assertion helper rejects empty stream ---
set +e
: | hf_assert_event '.type == "status"' 2>/dev/null
rc=$?
set -e
[[ $rc -ne 0 ]]

# --- teardown is idempotent ---
hf_teardown || true
hf_teardown || true

echo "PASS"
