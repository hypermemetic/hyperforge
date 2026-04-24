---
id: V5REPOS-2
title: "ForgePort capability — portable metadata read/write per D3"
status: Ready
type: implementation
blocked_by: [V5CORE-3, V5CORE-4, V5CORE-7, V5CORE-9]
unlocks: [V5REPOS-9, V5REPOS-10, V5REPOS-11, V5REPOS-13, V5REPOS-14, V5REPOS-15]
---

## Problem

Every provider adapter (GitHub, Codeberg, GitLab) and every downstream
metadata flow (`repos.sync`, `repos.push`) must agree on one capability
surface. Without a pinned intersection, adapters drift and sync/push
quietly lose fields. This ticket pins the capability — method set, inputs,
outputs, error classes — as the contract every adapter conforms to and
every metadata consumer relies on. Per D3, the intersection is
`{default_branch, description, archived, visibility}`.

## Required behavior

Two capability methods MUST be defined. Both take the adapter's
authenticated handle (credentials resolved via the V5CORE-4 resolver from
the org's `CredentialEntry` list) and a target remote.

**Capability method: read metadata**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | adapter dispatched by `provider` (derived per V5REPOS-12) |
| `repo_ref` | `RepoRef` | yes | `org` + `name` from the registered repo entry |

| Output / Event | Shape | Notes |
|---|---|---|
| success | one metadata value whose field set is exactly `DriftFieldKind` members | `default_branch: String`, `description: String`, `archived: bool`, `visibility: String` |
| error | typed error event classifying failure | see error classes below |

**Capability method: write metadata**

| Input | Type | Required | Notes |
|---|---|---|---|
| `remote` | `Remote` | yes | |
| `repo_ref` | `RepoRef` | yes | |
| `fields` | map of `DriftFieldKind` → value | yes | only fields present are written; absent fields are untouched on the remote |

| Output / Event | Shape | Notes |
|---|---|---|
| success | echoed field set confirming what was applied | caller compares against request to detect partial writes |
| error | typed error event classifying failure | see error classes below |

**Error classes (closed set for v1).** `not_found` (remote repo absent),
`auth` (credential missing, invalid, or insufficient scope), `network`
(transport failure), `unsupported_field` (a requested write field is not
portable on this provider — MUST be impossible if fields are restricted
to `DriftFieldKind`), `rate_limited` (provider signalled throttling).

Edge cases:

- Provider-specific metadata fields (e.g., GitHub topics, GitLab merge
  methods) are OUT of scope for this trait. Adapters MAY expose them via
  provider-specific extensions, but the extensions are NOT members of
  this capability and NOT reachable through `repos.sync` / `repos.push`.
- `visibility` on providers that model it as a boolean (public/private)
  and those that model it as a tri-state (public/internal/private) both
  serialize as `String`; value validation is per-adapter.
- Reading a remote that does not exist returns `not_found`, never an
  empty success value.

## What must NOT change

- v4's adapter interfaces are not touched by this ticket.
- Per D3, the intersection is fixed at four fields for v1. Expanding
  it requires a new ticket, not an adapter-level addition.
- Per the Secret redaction rule, neither capability method ever returns
  a resolved credential value in its output.

## Acceptance criteria

1. A schema introspection surface exposes the two capability methods with the input/output shapes above; the response includes exactly the four `DriftFieldKind` fields in the metadata value.
2. No adapter ticket (V5REPOS-9/10/11) can pass its own acceptance without implementing both methods with these signatures.
3. The error-class set at the wire boundary is exactly the five classes listed; any other error variant is a hard failure at the wire boundary (closed-variant rule from §types).
4. A test exercising the capability through any landed adapter (tier 2) produces a metadata value whose top-level keys are exactly `default_branch`, `description`, `archived`, `visibility` — no extras, no omissions.
5. Attempting to write a key outside `DriftFieldKind` via the capability is rejected before any provider call is made.

## Completion

- Run `bash tests/v5/V5REPOS/V5REPOS-2.sh` → exit 0.
- Status flips in-commit with the implementation.
