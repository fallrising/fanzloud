---
id: ADR-0001
title: Infrastructure-only task contract exception
status: accepted
date: 2026-07-27
deciders: [project]
---

# Context

The Technical Design seeds T000 as the repository bootstrap task, while §9.3 and §10.3 originally
required every task to reference at least one Contract Unit. A Contract Unit describes a production
boundary with behavior, errors, effects, atomicity, and tests. T000 creates developer tooling and CI
interfaces but no production boundary, so assigning it a synthetic CU would misrepresent the
contract model.

# Decision

A bootstrap or repository-governance task may omit Contract Units only when it introduces no
production API, protocol, persisted format, trust boundary, or runtime behavior.

Such a task must:

- Identify its versioned developer and CI interfaces.
- Define exact machine-executable acceptance commands.
- State explicitly that it introduces no production boundary.
- Trace directly to the governing Technical Design or ADR clauses.
- Produce verification evidence and an acceptance report like every other task.

Any production boundary introduced by the work removes eligibility for this exception and requires
a Contract Unit and specification.

# Consequences

- T000 can satisfy the Definition of Ready without a fictional infrastructure CU.
- Toolchain manifests, workspace manifests, dependency policy, and CI commands are treated as
  compatibility-sensitive developer interfaces.
- T010 and all subsequent production work remain subject to the normal CU and specification rules.
- Traceability rows for qualifying tasks use `N/A — ADR-0001 infrastructure exception` for CU and
  rustdoc fields.

# Alternatives considered

## Create CU-INFRA-00

Rejected because it would require production-contract fields such as failure atomicity and public
boundary semantics for repository scaffolding.

## Leave T000 as an undocumented exception

Rejected because it would preserve a contradiction in the normative delivery protocol.

