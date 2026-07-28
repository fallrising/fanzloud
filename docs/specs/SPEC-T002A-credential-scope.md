---
id: SPEC-T002A
subject: T002A credential scope lease and isolation
status: verified
contract_units: [CU-AUTH-P0-02]
archetypes: [B, E]
atomicity: E1
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2, 2.3, 7.4, 8.2–8.10, 11.3 P14, 14, and 15.0
- ADR-0002 §Credential boundary
- T002A task document
- T010 domain values and errors

# Contract Boundary

`CredentialScope` validates and owns the administrator-configured local boundary used by the Codex
login broker: the pinned native executable, operator `CODEX_HOME`, trusted state directory, trusted
working directory, fixed child environment, and exclusive operation lease. It never starts Codex,
reads `auth.json`, parses provider output, or decides provider login state.

# Public Boundary

```rust
pub struct CredentialScope { /* private validated paths and command policy */ }
pub struct CredentialScopeLease<'scope> { /* private locked file */ }

impl CredentialScope {
    pub fn validate(config: CredentialScopeConfig)
        -> Result<Self, CredentialScopeError>;
    pub fn try_acquire(&self)
        -> Result<CredentialScopeLease<'_>, CredentialScopeError>;
}
```

Public getters expose only the command policy needed by T002B. They never expose credential bytes,
auth-cache contents, or a general-purpose command builder. Contract: CU-AUTH-P0-02.

# Preconditions and Disposition

All paths are administrator configuration, never browser input.

| Condition | Disposition | Failure |
|---|---|---|
| Linux P0 host | Checked | `UnsupportedPlatform` |
| Absolute, canonical, non-symlink native executable | Checked | `ExecutableUnsafe` |
| Executable is a regular file, owned by root or effective runner UID, executable, and not group/other writable | Checked | `ExecutableUnsafe` |
| `CODEX_HOME`, state, and working paths are existing canonical directories owned by effective UID with mode `0700` | Checked | `DirectoryUnsafe` |
| The three directories do not equal, contain, or nest within one another | Checked | `DirectoryOverlap` |
| Neither the executable nor a configured directory has a `.git` file or directory in its ancestry | Checked | `RepositoryPathRejected` |
| The fixed child environment contains only `CODEX_HOME` | Type/private construction | No public arbitrary environment input |
| Lease file is regular, non-symlink, effective-UID owned, and mode `0600` | Checked | `LeaseUnsafe` |

The path checks are re-run immediately before every lease acquisition and before T002B process
spawn. Validation does not convert a previously safe mutable host into a permanently trusted path.
`[NEW-SPEC]`

# Success and State Transition

`validate` has E0 behavior apart from bounded metadata reads. `try_acquire` creates or opens exactly
`<state_dir>/login.lock` with mode `0600`, verifies it, and obtains a non-blocking exclusive kernel
file lock. Success returns one move-only RAII lease. Dropping the lease releases the kernel lock.

Lease acquisition is E1: either the caller owns the exclusive lock or receives a typed failure. A
second caller receives `LoginAlreadyRunning` and must observe/retry later; it never steals, deletes,
or replaces the lock. A process crash releases the kernel lock, but does not alter T002B's durable
login ledger or make an uncertain login retryable.

# Exit Invariants

- At most one live `CredentialScopeLease` owns an operator scope across processes.
- No checked failure changes `CODEX_HOME`, the trusted working directory, or an existing lease
  file's contents or permissions.
- A newly created lease file is mode `0600` before it becomes observable to another caller.
- The scope offers only the exact native Codex executable plus fixed version/status/device-login
  argv defined by SPEC-T002B; it cannot launch a shell, repository command, `codex exec`, or
  browser-supplied argument.

# Concurrency, Crash, and Cleanup

The Linux kernel file lock is the cross-process serialization primitive. In-process ownership is
also enforced by Rust's move/borrow rules. A crash may release the lock, so T002B must reconcile its
ledger and parent-death-bound child before retry; CU-AUTH-P0-02 does not infer provider outcome from
lock availability.

The lease file is durable scope metadata, not a temporary secret, and is not removed on normal
release or crash. T002A creates no disposable working directory and never deletes `CODEX_HOME`.

# Security Contract

- The child command uses an absolute validated native binary, a trusted working directory,
  `env_clear`, and only `CODEX_HOME`.
- Fixed argv enforces ChatGPT login and file credential storage; browser input cannot change flags,
  paths, environment, issuer, client ID, or executable.
- Repository ancestry rejection covers `.git` files used by worktrees and `.git` directories.
- Public errors identify the failed policy class but do not include raw paths, environment values,
  file contents, or source-error text that could disclose secrets.
- This is the Linux single-VPS P0 boundary from TD §7.4. Other platforms fail closed before side
  effects. `[NEW-SPEC]`

# Non-Guarantees

- Administrator configuration and the trusted host remain trusted.
- `.git` ancestry inspection does not discover a repository whose marker was deliberately removed
  or disguised.
- T002A does not parse config files inside `CODEX_HOME`, inspect credentials, start a process,
  reconcile provider state, or protect against an already-compromised root account.
- P0 does not inherit proxy, custom-CA, keyring, or arbitrary host environment variables.

# Required Tests

| Clause | Required test |
|---|---|
| Safe paths and command policy | `credential_scope_accepts_private_non_repository_paths` |
| Unsafe `CODEX_HOME` mode | `login_home_permissions_are_rejected_when_unsafe` |
| Ownership and symlink checks | `credential_scope_rejects_wrong_owner_and_symlinks` |
| Repository ancestry | `credential_scope_rejects_repository_paths` |
| Directory overlap | `credential_scope_rejects_overlapping_directories` |
| Fixed argv/environment | `login_command_is_not_user_controlled` |
| Cross-process single writer | `login_scope_is_single_writer` |
| Unsafe existing lease metadata | `credential_scope_rejects_unsafe_lease_file` |
| Mutable host revalidation | `credential_scope_rechecks_paths_before_use` |
| Lease crash semantics | `released_lock_does_not_override_uncertain_ledger` |
| No repository execution | `regression_cloud_runner_never_executes_repository_code` |
| Secret-canary isolation | `credential_scope_never_reads_or_projects_auth_cache` |

# Traceability and Gaps

The boundary projects ADR-0002's credential isolation and TD CU-AUTH-P0-02 without changing INV-007.
Linux mode/ownership checks, fixed environment, non-overlap, and persistent lease-file details are
`[NEW-SPEC]` local derivations. No in-scope `[TD-GAP]` remains.

# Acceptance Evidence

The implementation is `crates/codebox-agent-codex/**`. Public methods project `CU-AUTH-P0-02` in
rustdoc. Local validation on 2026-07-27:

```text
cargo fmt --all -- --check
  passed
cargo test -p codebox-agent-codex --all-features
  passed: 1 unit + 12 contract/security tests
cargo clippy -p codebox-agent-codex --all-targets --all-features -- -D warnings
  passed
cargo clippy --workspace --all-targets --all-features -- -D warnings
  passed
cargo test --workspace --all-targets --all-features
  passed
cargo build --workspace --bins --all-features
  passed
cargo deny check
  passed: advisories, bans, licenses, sources
git diff --check
  passed
```

The first sandboxed `cargo deny check` attempt could not lock the read-only user advisory cache. The
same command was rerun with permission to use that cache and returned all four `ok` results.
