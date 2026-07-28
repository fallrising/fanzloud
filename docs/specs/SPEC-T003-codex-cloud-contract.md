---
id: SPEC-T003
subject: T003 pinned Codex Cloud contract adapter
status: ready
contract_units: [CU-AGT-P0-01]
archetypes: [D, F]
atomicity: E0
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2–2.3, 5.1, 8.2–8.10, 9.2–9.6, 11.3 P14–P15, 14, and 15.0
- ADR-0002 §§Credential boundary, First execution interface, and Initial repository boundary
- T003 task document
- Accepted T001 and T010
- Official Codex CLI `rust-v0.145.0` `cloud-tasks/src/{cli,lib,util}.rs` and
  `cloud-tasks-client/src/api.rs`

# Contract Boundary

T003 is an E0 contract adapter. It validates typed values, constructs fixed argv arrays, and decodes
already captured bounded process output. It does not start Codex, read `CODEX_HOME`, submit a task,
poll a provider, or write a side-effect ledger.

The original seed task mixed CU-AGT-P0-01 E0, CU-AGT-P0-02 E2, and CU-BKD-01 E3. TD §9.3 requires
those independently testable atomicity models to be split. CU-AGT-P0-02 and CU-BKD-01 move to the
T004 integration parent, which must be decomposed before implementation. `[NEW-SPEC]`

# Pinned Official Surface

Local `--help` inspection and the official `rust-v0.145.0` source establish:

- `codex --version` emits `codex-cli 0.145.0`.
- `codex cloud exec --env <ENV> --attempts 1 --branch <BRANCH> <PROMPT>` submits one task and, on
  success, emits exactly one browser URL on stdout.
- The default URL is `https://chatgpt.com/codex/tasks/<TASK_ID>`.
- `codex cloud status <TASK_ID>` emits three human-readable stdout lines. The first begins with one
  of `[PENDING]`, `[READY]`, `[APPLIED]`, or `[ERROR]`. Only `READY` exits zero; the other three exit
  one.
- `codex cloud list --env <ENV> --limit 20 --json` emits a JSON object containing `tasks` and a
  nullable pagination `cursor`. Each task contains the exact fields in the pinned fixture.
- `codex cloud diff <TASK_ID> --attempt 1` emits the selected raw unified diff to stdout.
- `cloud apply` exists but is forbidden because it mutates a local working tree.
- The upstream implementation may append diagnostic/account metadata to `error.log` in its current
  working directory. T004 must run only in the private trusted working directory from T002A and
  must never publish that file as an artifact. `[NEW-SPEC]`

The committed fixtures are synthetic, source-derived examples rather than live provider captures.
They contain no account ID, prompt from a real user, credential, private repository, or provider
task. A CLI version change requires fixture and specification review. `[NEW-SPEC]`

# Public Values

- `CloudEnvironmentId`: administrator configured, 1–256 UTF-8 bytes after trimming, with no NUL or
  ASCII control character and no leading hyphen.
- `CloudBranch`: administrator configured, 1–255 UTF-8 bytes after trimming, with no NUL or ASCII
  control character and no leading hyphen.
- `CloudPrompt`: browser input, 1–65,536 UTF-8 bytes, not all whitespace, with no NUL or ASCII
  control character except LF and TAB. The exact value `-` is rejected because the pinned CLI
  interprets it as a request to read stdin rather than as a literal prompt.
- `CloudTaskId`: provider output, 1–128 ASCII bytes, begins with `task_`, and otherwise contains only
  ASCII alphanumeric, underscore, or hyphen.
- `CloudTaskUrl`: the exact HTTPS `chatgpt.com` origin and `/codex/tasks/<TASK_ID>` path, without
  userinfo, port, query, or fragment.
- `CloudCursor`: provider output, 1–2048 UTF-8 bytes with no NUL or ASCII control character.
- `CloudTaskStatus`: `Pending`, `Ready`, `Applied`, or `Error`.
- `CloudTaskSummary`: task ID, matching task URL, status, a title up to 4,096 UTF-8 bytes, RFC3339
  update time, optional environment ID/label up to 256 UTF-8 bytes each, at most 1,000,000 changed
  files, at most 1,000,000,000 added or removed lines, review flag, and optional attempts 1–4.
- `CloudTaskListPage`: at most 20 summaries plus an optional cursor.
- `CloudDiff`: at most 2 MiB of valid UTF-8, projected as untrusted display text and never applied
  locally by this task.

These numeric and character-set bounds are local defensive limits. `[NEW-SPEC]`

Strong constructors validate every external or administrator value. `Debug` for prompt, cursor,
task URL, title-bearing summaries, raw diff, and captured-output containers is redacted or omitted.
Public typed errors contain a category and field name only, never the rejected value or raw provider
output. `[NEW-SPEC]`

# Fixed Invocation Policy

The only commands are:

```text
codex --version
codex cloud exec --env=<ENV> --attempts=1 --branch=<BRANCH> -- <PROMPT>
codex cloud status <TASK_ID>
codex cloud list --env=<ENV> --limit=20 --json [--cursor=<CURSOR>]
codex cloud diff --attempt=1 <TASK_ID>
```

The version decoder reuses the accepted T002B pin and login fixture contract rather than claiming a
second independent live version capture. `[NEW-SPEC]`

Arguments are a typed ordered vector passed directly to a future `Command`; no shell string or
general argument extension exists. Exec always uses one attempt. The environment and branch can
only come from administrator configuration. The prompt is one argv item even if it contains spaces
or shell metacharacters. Every option/value pair uses one `--flag=value` argv item, so a
provider-supplied cursor can never become a separate option. Task ID and cursor can only come from
validated provider projections. Status and diff task IDs are safe bare positionals because the
validated `task_` prefix makes a leading option marker impossible.

The no-leading-hyphen rule for administrator environment and branch values is defense in depth and
produces an earlier configuration error even though `--flag=value` already prevents option
splitting. `[NEW-SPEC]`

T003 exposes no executable path, environment map, working directory, process builder, or execution
method. T004 must combine this policy with the accepted T002 credential scope, cleared environment,
private trusted working directory, bounded process supervision, and a durable E2 ledger.
`[NEW-SPEC]`

# Decoder Contract

All decoders receive stdout, stderr, overflow markers, and an optional exit code from a future
bounded runner.

- Every stream is at most 2 MiB for diff and 64 KiB for version, exec, status, and list. An overflow
  marker fails even if retained bytes otherwise parse.
- Version requires exact stdout `codex-cli 0.145.0\n`, empty stderr, and exit zero.
- Exec requires empty stderr, exit zero, one LF-terminated exact HTTPS URL at the pinned
  `chatgpt.com` origin and `/codex/tasks/<TASK_ID>` path, with no query, fragment, userinfo, port, or
  extra line.
- Status requires empty stderr, exactly three LF/CRLF-normalized nonempty lines, no ANSI/control
  characters, one exact status label, bounded ignored display fields, and the pinned exit mapping:
  `Ready → 0`; `Pending|Applied|Error → 1`. A recognized label paired with the wrong exit code is
  provider drift and fails closed.
- List requires empty stderr, exit zero, valid UTF-8 JSON matching the exact pinned schema with
  unknown fields denied, at most 20 tasks, unique task IDs, pinned status strings, RFC3339
  timestamps, bounded strings/counts, attempts 1–4, a validated optional cursor, and a pinned task
  URL whose embedded ID equals the row's `id`.
- Diff requires empty stderr and exit zero. It accepts an empty diff or valid UTF-8 up to 2 MiB, but
  rejects NUL and non-LF/TAB ASCII controls. It does not interpret, normalize, persist, or apply the
  diff.
- A nonzero exec/list/diff exit, missing exit status, nonempty stderr, invalid UTF-8, malformed
  output, or schema drift returns a typed redacted error.

`[NEW-SPEC]`

# Archetype D and F Answers

- Ordering: T003 decodes completed stdout and stderr independently. Bytes within each stream must
  retain source order; no cross-stream ordering is inferred.
- Termination: a decoder runs only after the future T004 supervisor has a terminal optional exit
  status and bounded capture. Every successful T003 decode requires the exact concrete exit code
  specified above.
- Cancellation: T003 has no live process or cancellation operation. T004 owns cancellation and must
  classify any partial submit as E2 `OutcomeUnknown`.
- Chunk-boundary invariance: capture chunking is irrelevant after byte-preserving concatenation.
  Every decoder result must be identical for every partition of the same per-stream bytes.
- Backpressure: T003 allocates only within the declared completed-capture bounds. T004 must continue
  draining both live streams after its retained bound and terminate/reap the process.
- Framing and partial data: each command has one whole-capture frame. A missing exit, overflow
  marker, incomplete URL/JSON/status template, or partial UTF-8 sequence is a typed failure; no
  partial public value is returned.
- Versioning and compatibility: only the exact pinned version/schema is accepted. Unknown fields or
  enum values fail closed and require a new fixture/specification review.
- Redaction: prompt, title, cursor, URL, diff, raw bytes, and stderr are not copied into typed errors
  or debug output.

The live ordering, cancellation, and backpressure mechanisms are intentionally T004 concerns, not
unanswered T003 behavior. `[NEW-SPEC]`

T004 must capture status stdout through a pipe, not a TTY. The pinned CLI then emits the
non-colorized template; if ANSI nevertheless appears, T003 rejects it as provider drift.
`[NEW-SPEC]`

# Security and Failure Semantics

Captured CLI bytes are untrusted. Validation and allocation are bounded before projection. No
decoder error includes raw output, prompt, title, cursor, task URL, diff, path, account metadata, or
credential material.

T003 has E0 atomicity and performs no retry. Its result only describes a fixed invocation or decoded
capture. T004 owns intent-before-submit, process execution, timeout/cancellation, provider
reconciliation, P15 unknown-submit behavior, and all retry decisions.

The `retriable: false` marker means T003 exposes no retrying operation or policy. Re-evaluating a
pure decoder with identical completed bytes is deterministic and has no external effect, but cannot
repair malformed provider output. `[NEW-SPEC]`

# Non-guarantees

- T003 does not prove the operator is authenticated, that an environment or branch exists, or that
  a task belongs to the configured environment.
- It does not guarantee stable Cloud output across CLI versions.
- It does not semantically validate patch paths or diff applicability.
- It does not protect against a compromised trusted CLI binary or host.
- A valid fixed invocation is not authorization to execute it.
- Provider-generated titles containing line breaks can make the human status template fail closed;
  list JSON remains the structured inspection surface.

# Required Test Skeletons

| Clause | Required test |
|---|---|
| Exact version pin | `cloud_version_fixture_is_exact` |
| Exec URL and task-ID parsing | `cloud_exec_fixture_is_exact` |
| Exec origin/path drift | `cloud_exec_rejects_origin_and_path_drift` |
| Four status/exit combinations | `cloud_status_exit_mapping_is_exact` |
| Status control/output drift | `cloud_status_rejects_control_and_template_drift` |
| Exact list JSON schema | `cloud_list_fixture_is_exact` |
| List bounds, duplicates, and unknown fields | `cloud_list_rejects_schema_and_bound_violations` |
| List URL and row-ID consistency | `cloud_list_rejects_url_id_mismatch` |
| Diff bound and controls | `cloud_diff_is_bounded_and_untrusted` |
| Per-stream chunk partition invariance | `cloud_decoders_are_chunk_partition_invariant` (property-based) |
| Typed value boundaries | `cloud_values_reject_unsafe_boundaries` |
| Prompt remains one argv item | `cloud_prompt_cannot_inject_argv` |
| No apply/local execution surface | `cloud_command_policy_has_no_apply_or_local_exec` |
| Redacted errors and debug | `cloud_decoder_errors_and_debug_are_redacted` |
| Missing terminal exit fails closed | `cloud_decoders_reject_missing_exit_status` |
| P14 no repository execution | `regression_cloud_runner_never_executes_repository_code` |
| P15 no retry authority | `regression_decoder_cannot_retry_unknown_cloud_submit` |

# Acceptance

T003 becomes Ready only after this new technical design receives the required fresh Claude review
with no blocking gap.

The T003 P14/P15 tests are narrow proofs that this E0 policy exposes neither repository execution
nor retry authority. T004 still owes P14 coverage over its real process launcher and the TD-exact
`regression_unknown_cloud_submit_reconciles_before_retry` test for the E2 lifecycle.

# Design Review

Claude Code 2.1.220 rejected the first draft because the public list projection omitted the pinned
`url` field, `CloudPrompt` allowed the upstream `-` stdin sentinel, and the Archetype D questions
were not assigned. This revision adds a validated task URL with row-ID consistency, rejects that
sentinel, specifies injection-resistant option/value argv, and explicitly assigns completed-capture
versus live-stream responsibilities. A second review found colorized spacing in two non-TTY status
fixtures; this revision corrects those summary lines and clarifies that T004 retains the full P14
and P15 regressions. A third review required the TD §11.1 property-based chunk-partition skeleton
and explicit numeric ceilings for diff statistics; both are now normative, with additional
missing-exit and positional-injection traceability.

The fourth fresh, read-only review returned `DESIGN ACCEPTED` with no blocker after independently
checking the official `rust-v0.145.0` source, every fixture, CU/atomicity split, D/F questions,
mechanical test mapping, bounds, redaction, argv policy, and P14/P15 ownership.
