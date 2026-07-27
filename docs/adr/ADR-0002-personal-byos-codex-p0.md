---
id: ADR-0002
title: Personal BYOS Codex P0
status: proposed
date: 2026-07-27
deciders: [project]
---

# Context

The project owner wants to operate their existing coding-agent subscriptions from a private cloud
host. The original task order prioritizes the native P1 foundation and would delay a usable external
agent. TD §1.6 also forbids repackaging consumer subscription sessions, while INV-007 originally
placed every provider credential exclusively in the control plane.

Official Codex documentation supports:

- ChatGPT sign-in for subscription-backed Codex usage.
- `codex login --device-auth` for remote or headless devices.
- Per-user `CODEX_HOME` state containing authentication, configuration, logs, and sessions.
- Isolated provider-hosted execution through experimental `codex cloud` commands.
- Product integration through `codex app-server`, whose command and WebSocket transport are
  currently experimental.

An unmodified Codex CLI must read its own credential state. Local `codex exec` also launches
repository-controlled tool subprocesses, and the documented workspace sandbox must not be relied on
to protect secrets from reads. The first slice therefore delegates repository execution to Codex
Cloud rather than running it beside `CODEX_HOME`.

# Decision

## Deployment and commercial boundary

P0 supports personal BYOS only:

- The deployment is private and has one operator.
- The operator authenticates their own provider account through the provider's official CLI.
- The operator remains subject to the provider's plan limits and terms.
- Codebox does not pool, share, lend, resell, proxy for other users, or advertise access to the
  subscription.
- Multi-user or public deployment is blocked until a later ADR defines provider authorization,
  tenancy, revocation, abuse controls, and a stronger secret boundary.

Codex is the first supported subscription adapter. Claude Code and Cursor Agent are separate later
tasks because their authentication and event contracts differ. Grok subscription support is not
claimed without an official coding-agent CLI and compatible authorization contract.

## Credential boundary

P0 adds a dedicated trusted agent-runner credential domain:

- Each operator has an independently permissioned `CODEX_HOME` outside repositories and disposable
  workspaces.
- Authentication is performed by the operator using the official Codex login flow.
- The browser may receive login status and provider-issued verification instructions, but never an
  access token, refresh token, API key, or `auth.json` content.
- Credentials are never placed in prompts, argv, general process environment, logs, events,
  artifacts, diffs, or downloadable files.
- The trusted Codex CLI process may read its credential directory only to authenticate and operate
  Codex Cloud tasks.
- The trusted runner never checks out a repository and never launches repository-controlled tools.
  Those actions occur in an OpenAI-managed Codex Cloud environment that does not receive the local
  `CODEX_HOME`.
- Credential isolation and redaction require explicit security tests with canary secrets.

This changes INV-007: provider secrets may exist in an ADR-approved trusted agent runner, but never in
the workspace/tool domain.

## First execution interface

The first vertical slice uses pinned official Codex CLI `0.145.0` and its experimental `codex cloud`
commands:

- The executable version is checked at startup.
- Task submission uses `codex cloud exec --env <configured-id> --attempts 1 --branch <configured-ref>`
  with arguments passed as an argv array; prompts are never interpolated into a shell command.
- Task status, list, and diff output are normalized behind a provider-neutral adapter.
- The environment ID and repository branch are administrator configuration, not browser-controlled
  host paths or arbitrary repository URLs.
- A submission interrupted before its provider task ID is durably recorded is `OutcomeUnknown` and
  must be reconciled through bounded task listing before any retry.
- Unknown or malformed CLI output produces typed, redacted errors rather than panics.
- The exact CLI version and captured output fixtures are contract-tested because the cloud command
  surface is experimental.

This interface does not provide local tool approvals, local sandbox control, token-level streaming,
or a stable upstream protocol. P0 explicitly exposes those non-guarantees. A later task may adopt a
documented stable provider API if one becomes available.

## Initial repository boundary

The first slice operates only on an administrator-configured Codex Cloud environment and branch.
Repository connection, permissions, setup, secrets, and network policy are configured directly in
Codex Cloud by the operator. Browser-provided environment IDs, arbitrary Git URLs, private Git
credentials, submodules, hooks, local checkout, and push are out of scope.

## P0 user-visible result

The private web flow is:

```text
Check Codex login
→ complete official login if needed
→ enter a prompt
→ submit a task to an administrator-configured Codex Cloud environment
→ watch normalized task status
→ view the final provider-generated diff
```

# Consequences

- T001 and the reusable T010 foundation unlock T002–T007 before the remaining P1 tasks.
- P0 yields a usable personal cloud agent sooner than the full native P1 architecture.
- P0 is not safe or licensed as a general multi-tenant subscription gateway.
- The credential runner becomes trusted infrastructure but executes no repository-controlled code.
- Interactive approvals, durable replay, private repositories, multiple providers, and team use
  remain later work.

# Alternatives considered

## Use provider API keys only

Rejected as the sole P0 path because it does not meet the owner's subscription-backed use case.
API-key adapters remain supported later.

## Use local Codex exec or app-server first

Rejected for the first slice. Both execute repository-controlled commands near local credential
state, and the documented local sandbox must not be treated as a secret-read boundary.

## Mount credentials into an ordinary workspace container

Rejected because repository content and tool subprocesses could read or exfiltrate the credential.

## Share one subscription among Codebox users

Rejected because it violates the personal BYOS boundary and the existing prohibition on repackaging
consumer quota.
