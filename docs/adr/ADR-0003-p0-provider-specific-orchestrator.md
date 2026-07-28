---
id: ADR-0003
title: Provider-specific P0 orchestrator and managed-state E0 scope
status: accepted
date: 2026-07-28
deciders: [project]
---

# Context

T004 decomposition exposed two conflicts in the P0 fast path.

First, TD §15.0 assigned generic `AgentBackend` conformance (CU-BKD-01) to T004 even though the
generic port's public context, event-sink, handle, capability, approval, and error contracts belong
to T020/T180. Those tasks are deliberately outside ADR-0002's provider-specific fast path.
Defining a second minimal P0 backend port would create a compatibility-sensitive interface that
T180 might later replace.

Second, CU-CLOUD-P0-02 classifies diff retrieval as E0 while ADR-0002 authorizes the official CLI
to operate its own `CODEX_HOME`. Even a provider read may maintain or refresh provider-owned
credential state. Codebox must not inspect credential bytes merely to prove an application-level
read contract.

The pinned `0.145.0` Cloud implementation also attempts to append account/diagnostic metadata to a
cwd-relative `error.log` during backend initialization. That Codebox-controlled write can and
should be prevented independently.

# Decision

## Keep P0 provider-specific

T004 contains three children only:

- T004A — CU-CLOUD-P0-01 E2 trusted submit/status/list runner.
- T004B — CU-AGT-P0-02 E2 Cloud task lifecycle.
- T004C — CU-CLOUD-P0-02 E0 Cloud diff retrieval.

CU-BKD-01 remains owned by T180, after T020 defines versioned canonical events. T005 may consume
the accepted provider-specific T004 lifecycle directly for the private Codex-only P0. It must not
name that adapter `AgentBackend` or claim generic backend conformance.

This does not remove the TD §5.1 architecture target. It prevents the fast path from freezing a
second incompatible public port before its canonical dependent types exist.

## Define the E0 managed-state boundary

For CU-CLOUD-P0-02, E0 covers:

- provider task/repository state;
- the T004 submit ledger and lifecycle projection;
- lease metadata and task eligibility;
- the trusted working directory, including the diagnostic sentinel; and
- every other Codebox-managed event, artifact, log, configuration, and durable record.

E0 does not claim byte-identical provider-owned `CODEX_HOME`, remote access/audit logs, network
telemetry, or host access timestamps. Those domains are owned by the trusted official CLI,
provider, or operating system under ADR-0002. They remain isolated and are never read, diffed,
logged, served, or copied by Codebox.

T004C still performs no internal retry. This scope clarification does not authorize a retry after
an ambiguous submit or weaken INV-006.

## Prevent the pinned cwd diagnostic write

Before any pinned Cloud command, T004A creates and revalidates `working_dir/error.log` as a private
directory. The reviewed `0.145.0` helper opens that name with create+append and ignores open
failure; opening a directory as a file fails, so the append becomes a no-op. An existing file,
symlink, unsafe directory, or replacement fails closed before spawn. A CLI version change must
review every cwd-relative write before retaining this control.

# Consequences

- T004D is removed from the P0 graph; T004 depends on T004A–T004C.
- T005 can proceed after those three children and parent composition acceptance without waiting for
  T020/T180.
- T180 remains the one owner of CU-BKD-01 and the generic backend compatibility surface.
- T004C can test E0 by snapshotting the explicitly enumerated managed state without reading
  credential material.
- Credential refresh remains a trusted CLI concern, not a hidden Codebox side effect.
- The diagnostic directory sentinel is version-specific and must fail closed on drift.

# Alternatives Considered

## Pull T020 and T180 into P0

Rejected because it defeats the ADR-0002 fast path and couples a single-provider private slice to
the broader P1 event/backend foundation.

## Define a temporary P0 generic backend

Rejected because the missing canonical events and sink commit semantics are compatibility-sensitive.
A bridge or replacement would create two meanings of `AgentBackend`.

## Reclassify all diff retrieval as E2

Rejected because the provider task operation is a read and Codebox-managed state can remain
identical. Provider-owned credential maintenance is isolated by ADR-0002 and cannot be safely
inspected by Codebox.

## Allow the upstream diagnostic file inside the private cwd

Rejected because the exact pinned write can be prevented cheaply and may contain account metadata.
Isolation remains defense in depth, not the primary control.

# Review Evidence

The first fresh Cursor Agent review of the T004 design rejected a graph that still required T004D
while recommending its removal. It also confirmed that deferring CU-BKD-01 avoids a prematurely
frozen public port and that managed-state E0 scope is consistent with TD reads and ADR-0002. A
second fresh review is required before T004A becomes Ready.
