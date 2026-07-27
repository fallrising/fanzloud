# Traceability

| Requirement / Invariant | CU | Specification / Decision | Test or check | Code / Configuration | Evidence | Status |
|---|---|---|---|---|---|---|
| T000 Cargo workspace and CI baseline | N/A — ADR-0001 infrastructure exception | T000 task §Outcome | T000 machine acceptance | `Cargo.toml`, `.github/workflows/ci.yml` | Pending | Implementing |
| TD §3.1 four application boundaries build | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo build --workspace --bins --all-features` | `apps/*` | Pending | Implementing |
| TD §12 formatting, lint, and test gates | N/A — ADR-0001 infrastructure exception | T000 task §Machine Acceptance | `cargo fmt`, `cargo clippy`, `cargo test` | Workspace | Pending | Implementing |
| TD §15.2 dependency-policy baseline | N/A — ADR-0001 infrastructure exception | T000 task §Deliverables | `cargo deny check` | `deny.toml` | Pending | Implementing |
