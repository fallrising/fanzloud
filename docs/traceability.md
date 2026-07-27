# Traceability

| Requirement / Invariant | CU | Specification / Decision | Test or check | Code / Configuration | Evidence | Status |
|---|---|---|---|---|---|---|
| T000 Cargo workspace and CI baseline | N/A — ADR-0001 infrastructure exception | T000 task §Outcome | T000 machine acceptance | `Cargo.toml`, `.github/workflows/ci.yml` | Local suite at `76f72b3`; [hosted CI run 30260756940](https://github.com/fallrising/fanzloud/actions/runs/30260756940) passed on `f9f3e2d` | Accepted |
| TD §3.1 four application boundaries build | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo build --workspace --bins --all-features` | `apps/*` | Passed at `76f72b3`; ACCEPT-T000 §Clause-to-Evidence | Verified locally |
| TD §12 formatting, lint, and test gates | N/A — ADR-0001 infrastructure exception | T000 task §Machine Acceptance | `cargo fmt`, `cargo clippy`, `cargo test` | Workspace | Passed at `76f72b3`; ACCEPT-T000 §Clause-to-Evidence | Verified locally |
| TD §15.2 dependency-policy baseline | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo deny check` | `deny.toml` | Passed at `76f72b3`; advisories, bans, licenses, sources all `ok` | Verified locally |
| P0 personal BYOS is operator-owned and never pooled or resold | P0 CU inventory | ADR-0002 §Deployment and commercial boundary | T001 documentation consistency review | TD §1.6–§1.7 | Claude content review passed; hosted T000 prerequisite [run 30260756940](https://github.com/fallrising/fanzloud/actions/runs/30260756940) passed | Verifying |
| INV-007 credential runner remains separate from provider-managed repository execution | CU-AUTH-P0-02, CU-CLOUD-P0-01 | ADR-0002 §Credential boundary | T004 credential-canary and no-local-execution suite | Future T002/T004 implementation | Claude content review passed; implementation pending | Proposed |
| P0 browser-to-Codex subscription vertical slice | All P0 CUs | TD §1.7; T001–T007 | T007 deterministic and live E2E | Future P0 implementation | Not implemented | Proposed |
| T010 strong IDs, paths, and base errors | CU-FS-00 | SPEC-T010; TD §16.1 | T010 acceptance suite | `crates/codebox-domain/**` | All local executable checks passed; fresh acceptance review unavailable | Verifying |
