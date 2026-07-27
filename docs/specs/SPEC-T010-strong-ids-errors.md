---
id: SPEC-T010
subject: T010 strong IDs and base errors
status: verified
contract_units: [CU-FS-00]
archetypes: [A, B, F]
atomicity: E0
retriable: true
---

# Normative Inputs

- TD §4.1, §4.3, §8.2–§8.10, and §16.1
- T010 task document
- ADR-0001

# Contract Boundary

The `codebox-domain` crate exposes validated value types only. It does not read or write the
filesystem, network, environment, clock, process state, or persistent storage.

The entity types are `SessionId`, `TurnId`, `ToolCallId`, `ApprovalId`, `SandboxId`, `ArtifactId`,
and `CommandId`. `EventSeq` represents a session-stream sequence number. `WorkspacePath` is a
portable workspace-relative path string.

# Preconditions

- UUID input to an ID conversion may be any UUID, including nil; nil is rejected with `IdError`.
- Workspace path input is a UTF-8 string. Empty input, NUL, absolute or drive-prefixed input,
  parent traversal, backslash separators, and an empty normalized path are rejected.
- `EventSeq::checked_next` may be called at any value; the maximum value returns a typed overflow
  error.

No caller-controlled precondition is represented as an unchecked internal assertion.

# Success Postconditions

## Entity IDs

- `new()` returns a non-nil UUID-backed value.
- `try_from_uuid` accepts non-nil UUIDs and preserves their identity.
- Serialization and deserialization preserve identity.
- Deserialization rejects nil UUIDs, so serialization cannot reintroduce an invalid ID.
- The seven entity types have identical representation behavior but are distinct Rust types.

## WorkspacePath

- `try_new` returns a non-empty normalized slash-delimited path.
- `.` components and repeated slash separators are removed.
- `..` components are rejected, including those that would remain within the workspace.
- The normalized UTF-8 representation is at most 4096 bytes.
- Serialization emits the normalized string; deserialization applies the same validation.
- Reapplying `try_new` to `as_str()` returns an equal value.

`[NEW-SPEC]` The 4096-byte normalized UTF-8 bound is selected as a portable bounded-resource
limit for this foundation. It is not a host filesystem limit.

## EventSeq

- `initial()` is zero.
- `value()` returns the exact stored sequence number.
- `checked_next()` increments every value below `u64::MAX` and returns `EventSeqError::Overflow`
  at the maximum without wrapping.

## Domain errors

- `DomainError` groups only value-construction failures defined by this specification.
- Error displays contain type and validation information, never input payloads or secrets.
- Each checked error tells the caller to correct the value or perform the required bounded recovery;
  no error silently changes input semantics.

# Exit Invariants

On success, checked failure, serialization failure, and deserialization failure:

- No external state is changed.
- No invalid value is returned.
- No panic is used for caller-controlled input.
- Normalization is deterministic and repeatable.

# Failure Atomicity and Retry

This is an E0, pure/bounded-value boundary. It has no partial side effect or commit point.
Repeating a validation call is safe; retrying an invalid input without changing it is guaranteed to
return the same error class.

# Concurrency, Cancellation, Timeout, and Crash

The operations are synchronous and do not own shared mutable state, so concurrent calls are
independent. Cancellation and timeout do not apply. A process crash loses only the in-memory value
being constructed; no durable state was changed and recovery is a caller concern.

# Security and Trust Boundary

The constructors treat strings, UUIDs, and serialized values as untrusted input. They reject path
shapes that could cross the workspace boundary at the value layer. `WorkspacePath` does not inspect
symlinks, resolve a filesystem path, prevent later string concatenation, or authorize an operation.
Those guarantees belong to the filesystem execution boundary.

# Non-guarantees

- `WorkspacePath` does not guarantee symlink containment or filesystem existence.
- An ID does not prove ownership, authorization, persistence, or uniqueness across independent
  processes beyond the UUID construction contract.
- `DomainError` does not replace boundary-specific errors for storage, network, provider, or
  sandbox operations.

# Observability

No logs, metrics, events, artifacts, or audit records are emitted. Callers may inspect the typed
error and decide what boundary-level telemetry is appropriate without exposing input payloads.

# Required Tests

| Specification clause | Executable test |
|---|---|
| Every ID is non-nil | `ids_are_non_nil` |
| Nil IDs are rejected | `nil_ids_are_rejected` |
| IDs round-trip through serde | `ids_round_trip_through_serde` |
| Nil IDs cannot deserialize | `nil_id_deserialization_is_rejected` |
| ID types are not interchangeable | compile-fail doctest `id_types_are_not_interchangeable` |
| Paths normalize deterministically and idempotently | `workspace_path_normalizes_and_is_idempotent` |
| Malicious path shapes are rejected | `workspace_path_rejects_boundary_inputs` |
| Path serde revalidates input | `workspace_path_deserialization_revalidates` |
| Path length is bounded | `workspace_path_rejects_overlong_input` |
| Sequence overflow does not wrap | `event_seq_overflow_is_typed` |
| Invalid inputs do not panic | `malformed_values_return_errors_without_panicking` |

# Acceptance Evidence

On 2026-07-27, the implementation passed:

- `cargo fmt --all -- --check`
- `cargo test -p codebox-domain --all-features` — 10 integration tests and one compile-fail
  doctest passed.
- `cargo clippy -p codebox-domain --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-targets --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo build --workspace --bins --all-features`
- `cargo deny check` — advisories, bans, licenses, and sources all `ok`.

Hosted CI run [30262687153](https://github.com/fallrising/fanzloud/actions/runs/30262687153)
also passed all workflow gates on commit `aa56b75`.

The independent acceptance report is conditional because the requested fresh Claude document
review did not return a report. No `[TD-GAP]` remains in the implementation scope.
