# Codebox

[![CI](https://github.com/fallrising/fanzloud/actions/workflows/ci.yml/badge.svg)](https://github.com/fallrising/fanzloud/actions/workflows/ci.yml)

Codebox is a provider-neutral cloud coding agent platform written in Rust. It is designed to run
coding work in isolated per-session sandboxes and expose the workflow through a web interface.

The currently implemented milestone is the accepted **P0 personal BYOS** slice: a private,
single-operator web control layer for the operator's own ChatGPT/Codex subscription. Codebox uses a
pinned official Codex CLI to submit work to an administrator-configured Codex Cloud environment,
stream normalized task state, and display the resulting diff. Repository-controlled work stays in
the provider-managed environment and never runs beside the local credential directory.

> [!IMPORTANT]
> P0 is for a private, single-operator deployment. It must not be used to pool, share, resell, or
> expose a consumer subscription as a multi-user service. See
> [ADR-0002](docs/adr/ADR-0002-personal-byos-codex-p0.md) for the complete boundary.

## Current status

The full T001–T007 personal-BYOS path plus the T010 and T020 domain foundations are accepted. The
deterministic Linux end-to-end test covers the browser, HTTP and WebSocket APIs, session runtime,
trusted Codex runner, and a fake provider process. The credential-gated live operator smoke has
not been run.

P1 development is underway. T030A adds the first SQLite durable-event boundary: private schema
initialization and atomic expected-sequence append. Replay, snapshots, native agents, local sandbox
execution, multi-user authentication, and the `node-agent`/`boxd` runtime remain future tasks.

See [the development handoff](docs/HANDOFF.md) and
[traceability matrix](docs/traceability.md) for the exact implementation and acceptance state.

## P0 flow

```text
Private browser
  │  HTTPS + authenticated WebSocket
  ▼
Control plane
  │  commands, snapshots, replay, and live events
  ▼
Process-lifetime session runtime
  │  typed lifecycle operations
  ▼
Trusted Codex runner
  │  pinned CLI, private CODEX_HOME, durable Cloud ledgers
  ▼
Codex Cloud managed environment
     repository work and final diff
```

The browser cannot select an executable, credential path, environment, branch, repository, or
apply operation. The trusted runner supports only the reviewed version, login, submit, status,
list, and diff command surfaces. Local `codex exec` and `codex cloud apply` are outside the P0
authority.

## Workspace

| Path | Status | Responsibility |
| --- | --- | --- |
| `apps/control-plane` | Implemented | Private HTTP API, replay-then-live WebSocket, and embedded operator page |
| `crates/codebox-session-runtime` | Implemented | One process-lifetime session, one active turn, ordered events, cancel, and recovery |
| `crates/codebox-agent-codex` | Implemented | Credential scope, device login, pinned Cloud adapter, ledgers, lifecycle, and diff retrieval |
| `crates/codebox-domain` | Implemented | Strong identifiers, paths, sequences, and base errors |
| `crates/codebox-event-store` | Implemented | Private SQLite initialization and atomic expected-sequence event append |
| `apps/node-agent` | Scaffold | Future host sandbox controller |
| `apps/boxd` | Scaffold | Future in-sandbox process and filesystem service |
| `apps/codebox-cli` | Scaffold | Future command-line client |

The target architecture and its trust boundaries are defined in
[the Technical Design](docs/TD.md).

## Prerequisites

Development and CI use:

- Rust `1.97.1` with `rustfmt` and Clippy (selected by `rust-toolchain.toml`)
- Node.js `24.18.0` for the dependency-free browser tests
- `cargo-deny 0.19.4` for the full dependency-policy gate
- Linux for the P0 credential runner and its end-to-end tests

Install the CI-pinned policy checker when you need the complete local gate:

```bash
cargo install cargo-deny --version 0.19.4 --locked
```

## Build and test

Run the CI validation plus the repository's local diff check:

```bash
node --test --test-isolation=none apps/control-plane/web/p0-client.test.mjs
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --workspace --bins --all-features
cargo deny check
git diff --check
```

The focused deterministic P0 acceptance test is:

```bash
cargo test -p codebox-control-plane --test p0_subscription_e2e \
  --all-features -- --exact \
  p0_subscription_e2e_fake_codex_reaches_final_diff
```

It uses a private fake Codex executable and does not require network access or real credentials.

## Run the private P0 control plane

Running against the live provider requires:

- Linux and the exact official `codex-cli 0.145.0` executable
- an operator-owned `CODEX_HOME`, ready for the official ChatGPT device-login flow
- an administrator-configured Codex Cloud environment and branch
- private HTTPS termination in front of the control-plane listener

The executable must be an absolute, canonical, non-symlink regular file owned by root or the
runner, executable, and not group/other writable. `CODEX_HOME`, the state directory, and the
working directory must be separate existing canonical directories, owned by the runner, mode
`0700`, outside every Git repository, and neither nested nor overlapping.

Configure the process with these administrator-only variables:

| Variable | Meaning |
| --- | --- |
| `CODEBOX_CODEX_EXECUTABLE` | Absolute path to the validated Codex `0.145.0` executable |
| `CODEBOX_CODEX_HOME` | Private Codex credential directory |
| `CODEBOX_STATE_DIR` | Private durable runner-ledger directory |
| `CODEBOX_WORKING_DIR` | Private, non-repository CLI working directory |
| `CODEBOX_CLOUD_ENVIRONMENT` | Preconfigured Codex Cloud environment ID |
| `CODEBOX_CLOUD_BRANCH` | Administrator-selected repository branch |
| `CODEBOX_PUBLIC_ORIGIN` | Canonical external HTTPS origin, with no path, query, or fragment |
| `CODEBOX_BOOTSTRAP_TOKEN` | Fresh opaque operator token, 32–128 bytes with no control characters |
| `CODEBOX_LISTEN_ADDRESS` | Internal socket address, normally behind private TLS termination |
| `CODEBOX_APP_SESSION_SECONDS` | Optional application session lifetime, 300–43,200 seconds |

After exporting the required values:

```bash
cargo run -p codebox-control-plane
```

Startup fails closed when a path, permission, origin, token, or provider boundary is invalid.
Each provider operation also verifies the pinned CLI contract and fails closed on version or output
drift. The binary intentionally returns only a generic startup error; use the specifications and
typed library errors when diagnosing administrator configuration.

## Delivery protocol

This repository is developed specification-first. Before changing production behavior:

1. Read [AGENTS.md](AGENTS.md) and the relevant part of [docs/TD.md](docs/TD.md).
2. Select exactly one Ready task and verify its Contract Units and dependencies.
3. Create or update the normative specification in `docs/specs/`.
4. Generate the named test skeletons before implementation.
5. Run the risk-appropriate gates and record evidence.
6. Update the task, acceptance report, rustdoc projection, handoff, and traceability matrix.
7. Obtain a fresh-context, document-driven acceptance review.

Normative artifacts live under:

- `docs/adr/` — accepted architecture decisions
- `docs/tasks/` — decomposed work and readiness
- `docs/specs/` — behavior, failure, security, and test contracts
- `docs/acceptance/` — independent acceptance decisions
- `docs/traceability.md` — requirement-to-evidence mapping

When documentation and implementation conflict, the higher-level normative document wins and the
affected task stops until the design is repaired.

## P0 non-goals

P0 does not provide public or multi-user SaaS, arbitrary repository selection, local repository
execution, interactive tool approval, automatic patch application or Git push, private Git
credential management, provider-task cancellation, or crash-durable browser/session replay.
