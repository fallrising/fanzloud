# Traceability

| Requirement / Invariant | CU | Specification / Decision | Test or check | Code / Configuration | Evidence | Status |
|---|---|---|---|---|---|---|
| T000 Cargo workspace and CI baseline | N/A — ADR-0001 infrastructure exception | T000 task §Outcome | T000 machine acceptance | `Cargo.toml`, `.github/workflows/ci.yml` | Clean-worktree suite at `76f72b3`; GitHub CI pending | Verifying |
| TD §3.1 four application boundaries build | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo build --workspace --bins --all-features` | `apps/*` | Passed at `76f72b3`; ACCEPT-T000 §Clause-to-Evidence | Verified locally |
| TD §12 formatting, lint, and test gates | N/A — ADR-0001 infrastructure exception | T000 task §Machine Acceptance | `cargo fmt`, `cargo clippy`, `cargo test` | Workspace | Passed at `76f72b3`; ACCEPT-T000 §Clause-to-Evidence | Verified locally |
| TD §15.2 dependency-policy baseline | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo deny check` | `deny.toml` | Passed at `76f72b3`; advisories, bans, licenses, sources all `ok` | Verified locally |
