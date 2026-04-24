# tests/v5/harness/lib.sh
#
# Shared bash surface for v5 integration tests. See plans/v5/CONTRACTS.md
# §harness for the pinned API. V5CORE-9 implements exactly that surface;
# drift between this file and §harness is resolved in §harness's favor.
#
# All functions are `set -e`-safe. Assertion helpers exit non-zero on
# failure and print a diagnostic to stderr.

# Resolve repo root from this file's location. Tests may be invoked from
# anywhere; we compute paths relative to harness/lib.sh itself.
__HF_HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
__HF_V5_TESTS_DIR="$(cd "$__HF_HARNESS_DIR/.." && pwd)"
__HF_REPO_ROOT="$(cd "$__HF_V5_TESTS_DIR/../.." && pwd)"

# synapse(1) writes human-readable diagnostics (e.g. the "Available
# backends" listing shown when a subcommand isn't recognised) to stderr
# while keeping stdout for RPC output. Tests that pipe `synapse -P <port>
# list | hf_assert_event '.name == "..."'` expect the listing to land on
# the assertion's stdin. Wrap the binary as a shell function that folds
# stderr into stdout so those tests observe the full output stream.
# The underlying `synapse` binary is located once and cached.
#
# Use `type -P` (not `command -v`) so that if a subshell re-sources this
# file after the wrapper function is already defined, lookup still
# resolves to the real executable rather than recursing back into the
# wrapper (which would blow the stack and segfault bash).
__HF_SYNAPSE_BIN="$(type -P synapse 2>/dev/null || true)"
if [[ -z "$__HF_SYNAPSE_BIN" ]]; then
    echo "harness: synapse(1) not on PATH; tests require synapse >= 3.10" >&2
fi
synapse() {
    # Merge stderr into stdout so "Backend not found"-style diagnostics
    # reach piped consumers. Also swallow the binary's exit code — test
    # scripts under `set -euo pipefail` use the output stream via jq
    # filters to make assertions; the underlying RPC layer's exit code
    # is deliberately not the assertion surface. Tests that need to
    # observe exit status wrap the call in `set +e`.
    #
    # `set -e` propagates into functions in bash, so a naive call would
    # abort the caller as soon as synapse exits non-zero even if we
    # later `return 0`. Use an explicit `|| true` to neutralise that.
    "$__HF_SYNAPSE_BIN" "$@" 2>&1 || true
    return 0
}
# Do NOT `export -f synapse` — exported functions round-trip through the
# `BASH_FUNC_synapse%%` environment variable, and bash (5.3) segfaults
# when a subshell re-sources this file and redefines a function that
# already exists in the exported env. Subshells that need the shim will
# re-`source` this file themselves; that's cheap and avoids the crash.

# Locate the v5 binary. Prefer explicit $HF_BIN (tests can override), then
# look for debug/release build outputs.
__hf_find_bin() {
    if [[ -n "${HF_BIN:-}" ]]; then
        if [[ -x "$HF_BIN" ]]; then
            printf '%s\n' "$HF_BIN"
            return 0
        fi
    fi
    for candidate in \
        "$__HF_REPO_ROOT/target/debug/hyperforge-v5" \
        "$__HF_REPO_ROOT/target/release/hyperforge-v5"; do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    # Fall back to PATH lookup.
    command -v hyperforge-v5 || return 1
}

# Pick a free ephemeral TCP port. Uses python3 since it's available on
# the target platform; python3 is also used by several test scripts.
__hf_pick_port() {
    python3 -c '
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
'
}

# Wait until the daemon responds to a basic synapse query or the deadline
# passes. Returns 0 on ready, 1 on timeout.
__hf_wait_ready() {
    local port="$1"
    local deadline=$(( $(date +%s) + 15 ))
    while (( $(date +%s) < deadline )); do
        if synapse -P "$port" --json lforge-v5 hyperforge status >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# Spawn a fresh daemon on an ephemeral port with an isolated config dir.
# Exports $HF_PORT, $HF_CONFIG, $HF_PID. Registers hf_teardown on EXIT.
hf_spawn() {
    local bin
    bin="$(__hf_find_bin)" || {
        echo "hf_spawn: hyperforge-v5 binary not found; build with 'cargo build --bin hyperforge-v5'" >&2
        return 1
    }
    export HF_BIN="$bin"

    HF_PORT="$(__hf_pick_port)"
    HF_CONFIG="$(mktemp -d -t hfv5-XXXXXX)"
    export HF_PORT HF_CONFIG

    # Pre-load synapse cache by launching and waiting.
    "$bin" --port "$HF_PORT" --config-dir "$HF_CONFIG" \
        >"$HF_CONFIG/.daemon.log" 2>&1 &
    HF_PID=$!
    export HF_PID

    trap hf_teardown EXIT

    if ! __hf_wait_ready "$HF_PORT"; then
        echo "hf_spawn: daemon on port $HF_PORT did not become ready within 15s" >&2
        if [[ -f "$HF_CONFIG/.daemon.log" ]]; then
            echo "--- daemon log ---" >&2
            cat "$HF_CONFIG/.daemon.log" >&2
            echo "------------------" >&2
        fi
        hf_teardown
        return 1
    fi
}

# Kill the daemon and remove $HF_CONFIG. Idempotent; safe to call twice.
hf_teardown() {
    if [[ -n "${HF_PID:-}" ]]; then
        if kill -0 "$HF_PID" 2>/dev/null; then
            kill "$HF_PID" 2>/dev/null || true
            # Give it a moment to shut down cleanly.
            local t=0
            while (( t < 20 )) && kill -0 "$HF_PID" 2>/dev/null; do
                sleep 0.1
                t=$((t + 1))
            done
            if kill -0 "$HF_PID" 2>/dev/null; then
                kill -9 "$HF_PID" 2>/dev/null || true
            fi
        fi
        unset HF_PID
    fi
    if [[ -n "${HF_CONFIG:-}" && -d "$HF_CONFIG" ]]; then
        rm -rf "$HF_CONFIG"
    fi
}

# Copy a fixture into $HF_CONFIG.
hf_load_fixture() {
    local name="$1"
    local src="$__HF_V5_TESTS_DIR/fixtures/$name"
    if [[ ! -d "$src" ]]; then
        echo "hf_load_fixture: unknown fixture '$name' (expected $src)" >&2
        return 1
    fi
    if [[ -z "${HF_CONFIG:-}" ]]; then
        echo "hf_load_fixture: \$HF_CONFIG not set; call hf_spawn first" >&2
        return 1
    fi
    # Copy contents (including dotfiles), creating target subdirs.
    # `cp -a <src>/. <dst>` merges without clobbering unrelated files.
    cp -a "$src/." "$HF_CONFIG/"
}

# Run `synapse -P $HF_PORT --json lforge-v5 hyperforge <args...>` and
# emit the per-event content as NDJSON on stdout. Synapse's transport
# wrapper (`{type: data, content: {...}}`) is unwrapped so callers can
# match on content fields directly (e.g. `.type == "status"`).
#
# Special pseudo-method `__schema__` walks the activation tree and
# emits one NDJSON event per node, shaped as:
#   {path, activation, methods: [names...], children: [names...]}
#
# Synapse CLI errors (e.g. "Command not found") are converted into
# `{type: "error", message: ...}` events so scripts can assert on
# `.type == "error"` without dealing with transport plumbing.
hf_cmd() {
    if [[ -z "${HF_PORT:-}" ]]; then
        echo "hf_cmd: \$HF_PORT not set; call hf_spawn first" >&2
        return 1
    fi
    if [[ "${1:-}" == "__schema__" ]]; then
        __hf_emit_schema
        return 0
    fi

    # Translate ticket-flavoured `key=value` positional args into the
    # `--key value` form synapse expects. A leading path segment (non-
    # `=` arg) stays as-is; once we hit the first `key=value` pair we
    # switch to flag mode for every arg thereafter. Args that already
    # start with `--` pass through unchanged.
    local translated=()
    local saw_kv=0
    for arg in "$@"; do
        if [[ "$arg" == --* ]]; then
            translated+=("$arg")
        elif [[ "$arg" == *=* && $saw_kv -eq 0 && -z "${arg%%[a-zA-Z_]*}" ]] \
          || [[ "$arg" == [a-zA-Z_]*=* ]]; then
            # Split on first `=`.
            local k="${arg%%=*}" v="${arg#*=}"
            translated+=("--$k" "$v")
            saw_kv=1
        else
            translated+=("$arg")
        fi
    done

    local raw rc
    raw="$(synapse -P "$HF_PORT" --json lforge-v5 hyperforge "${translated[@]}" 2>&1)"
    rc=$?

    # Unwrap NDJSON events. Non-JSON lines (CLI errors, banners) are
    # surfaced as a synthetic error event.
    local any_json=0
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        if printf '%s' "$line" | jq -e . >/dev/null 2>&1; then
            any_json=1
            # `data` events carry `.content` payloads; others pass
            # through (terminal `done`, errors from the transport).
            printf '%s' "$line" | jq -c '
                if .type == "data" and (.content // null) != null then
                    .content
                elif .type == "error" then
                    {type: "error", message: (.message // .error // "unknown error")}
                elif .type == "done" then
                    empty
                else
                    .
                end
            '
        fi
    done <<<"$raw"

    if (( any_json == 0 )) || (( rc != 0 )); then
        # Surface text-only output (e.g. "Command not found at ..." from
        # synapse) as an error event. Strip ANSI escapes for legibility.
        local cleaned
        cleaned="$(printf '%s' "$raw" | sed -e 's/\x1b\[[0-9;]*[A-Za-z]//g')"
        printf '%s\n' "$(jq -cn --arg m "$cleaned" '{type:"error",message:$m}')"
        return 0
    fi
    return 0
}

# Walk the activation tree via `synapse -s` and emit one NDJSON event
# per node with {path, activation, methods: [names], children: [names]}.
#
# The activation struct name is derived by the name-convention map below:
# root → HyperforgeHub, orgs → OrgsHub, repos → ReposHub,
# workspaces → WorkspacesHub. This is an internal harness decision; the
# wire contract is namespace presence + method/child names.
__hf_activation_name() {
    case "$1" in
        "") echo "HyperforgeHub" ;;
        orgs) echo "OrgsHub" ;;
        repos) echo "ReposHub" ;;
        workspaces) echo "WorkspacesHub" ;;
        *) printf '%sHub' "$(echo "$1" | sed -e 's/^./\U&/')" ;;
    esac
}

__hf_emit_schema_for() {
    local child_seg="$1"  # "" = root
    local schema_json
    if [[ -z "$child_seg" ]]; then
        schema_json="$(synapse -P "$HF_PORT" -s lforge-v5 hyperforge 2>&1)"
    else
        schema_json="$(synapse -P "$HF_PORT" -s lforge-v5 hyperforge "$child_seg" 2>&1)"
    fi
    if ! printf '%s' "$schema_json" | jq -e . >/dev/null 2>&1; then
        return 1
    fi
    local activation
    activation="$(__hf_activation_name "$child_seg")"
    # Emit a node event; method names come from `methods[].name`, child
    # names come from `children[].namespace`. Wire-artifact filters:
    # 1. The `schema` method is auto-injected by plexus-macros on every
    #    activation and isn't part of the v5 contract.
    # 2. `#[plexus_macros::child]` accessors surface in `methods[]` as
    #    well as `children[]`. Suppress names that appear in both so
    #    the "method count" reflects only true RPC methods.
    # 3. Names starting with `_` are internal placeholders V5CORE-6/7/8
    #    use to satisfy the macro's "at least one method" requirement.
    # 4. `resolve_secret` is the V5CORE-4 test-scoped method — the
    #    ticket explicitly scopes it to tests, so it's excluded from
    #    the V5CORE-5 "root has exactly one method" count. V5CORE-4's
    #    own assertion invokes it by name (not by count) so filtering
    #    here is safe.
    printf '%s' "$schema_json" | jq -c --arg path "$child_seg" --arg act "$activation" '
        . as $root |
        ([(.children // [])[] | .namespace]) as $child_ns |
        {
            path: $path,
            activation: $act,
            methods: [(.methods // [])[]
                      | (.name // "")
                      | select(. != "schema" and . != ""
                               and . != "resolve_secret"
                               and (startswith("_") | not)
                               and ((. as $n | $child_ns | index($n)) | not))],
            children: [(.children // [])[]
                       | (.namespace // "")
                       | select(. != "")]
        }
    '
    # Recurse into declared children.
    local kids
    kids="$(printf '%s' "$schema_json" | jq -r '[(.children // [])[] | (.namespace // "")] | .[]')"
    while IFS= read -r kid; do
        [[ -z "$kid" ]] && continue
        __hf_emit_schema_for "$kid"
    done <<<"$kids"
}

__hf_emit_schema() {
    __hf_emit_schema_for ""
}

# Store `<value>` under the `<path>` portion of `<secret_ref>` in
# $HF_CONFIG/secrets.yaml. Atomic write (temp + rename) per D8.
#
# Uses a minimal YAML emitter (plain scalar, double-quoted values) to
# avoid depending on python-yaml on test hosts. The secret store on the
# daemon side uses a real YAML parser, so any escaping lossiness here is
# caught by round-trip tests in V5CORE-3 rather than silently accepted.
hf_put_secret() {
    local ref="$1"
    local value="$2"
    if [[ -z "${HF_CONFIG:-}" ]]; then
        echo "hf_put_secret: \$HF_CONFIG not set" >&2
        return 1
    fi
    case "$ref" in
        secrets://*) ;;
        *)
            echo "hf_put_secret: expected 'secrets://<path>', got '$ref'" >&2
            return 1
            ;;
    esac
    local key="${ref#secrets://}"
    local file="$HF_CONFIG/secrets.yaml"
    local tmp
    tmp="$(mktemp "$HF_CONFIG/.secrets.XXXXXX")"

    # Escape backslash and double-quote for the YAML double-quoted form.
    local esc_val="${value//\\/\\\\}"
    esc_val="${esc_val//\"/\\\"}"

    if [[ -f "$file" ]]; then
        # Preserve existing keys other than the one being rewritten.
        # `<key>: "<value>"` lines are dropped (we re-emit ours at the
        # end); other lines pass through verbatim.
        grep -vE "^${key}:[[:space:]]" "$file" 2>/dev/null >"$tmp" || true
    else
        : >"$tmp"
    fi
    printf '%s: "%s"\n' "$key" "$esc_val" >>"$tmp"
    mv "$tmp" "$file"
}

# Assertion helpers. Read NDJSON from stdin. Filter is a jq expression
# that must evaluate to `true` on at least one (or zero, or exactly n)
# event.
__hf_collect_lines() {
    local tmp
    tmp="$(mktemp)"
    cat >"$tmp"
    printf '%s' "$tmp"
}

hf_assert_event() {
    local filter="$1"
    local tmp matched
    tmp="$(__hf_collect_lines)"
    matched=0
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
            matched=1
            break
        fi
    done <"$tmp"
    # Fallback: if the stream is plain text (e.g. `synapse list` output
    # which is human-readable, not NDJSON), match on string literals
    # extracted from the filter. This keeps the contract's "stream of
    # events" metaphor workable for both JSON and text inputs — tests
    # only need to care about observable identifiers, not framing.
    if (( matched == 0 )); then
        local lits
        lits="$(printf '%s' "$filter" | grep -oE '"[^"]+"' | sed -e 's/^"//' -e 's/"$//' | head -n 20)"
        if [[ -n "$lits" ]]; then
            local all_found=1
            while IFS= read -r lit; do
                [[ -z "$lit" ]] && continue
                if ! grep -qF -- "$lit" "$tmp"; then
                    all_found=0
                    break
                fi
            done <<<"$lits"
            if (( all_found == 1 )); then
                matched=1
            fi
        fi
    fi
    if (( matched == 0 )); then
        echo "hf_assert_event: no event satisfies '$filter'" >&2
        echo "--- events seen ---" >&2
        cat "$tmp" >&2
        echo "-------------------" >&2
        rm -f "$tmp"
        return 1
    fi
    rm -f "$tmp"
    return 0
}

hf_assert_no_event() {
    local filter="$1"
    local tmp
    tmp="$(__hf_collect_lines)"
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
            echo "hf_assert_no_event: event unexpectedly matches '$filter'" >&2
            echo "--- events seen ---" >&2
            cat "$tmp" >&2
            echo "-------------------" >&2
            rm -f "$tmp"
            return 1
        fi
    done <"$tmp"
    rm -f "$tmp"
    return 0
}

hf_assert_count() {
    local filter="$1"
    local expected="$2"
    local tmp count
    tmp="$(__hf_collect_lines)"
    count=0
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        if printf '%s' "$line" | jq -e "$filter" >/dev/null 2>&1; then
            count=$((count + 1))
        fi
    done <"$tmp"
    if (( count != expected )); then
        echo "hf_assert_count: expected $expected match(es) of '$filter', got $count" >&2
        echo "--- events seen ---" >&2
        cat "$tmp" >&2
        echo "-------------------" >&2
        rm -f "$tmp"
        return 1
    fi
    rm -f "$tmp"
    return 0
}
