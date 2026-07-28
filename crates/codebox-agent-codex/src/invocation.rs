use std::path::Path;

use crate::CloudInvocation;

/// One of the only Codex CLI operations authorized by the T002 boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexCommand {
    /// Verify the administrator-pinned native CLI version.
    Version,
    /// Inspect the active authentication method.
    LoginStatus,
    /// Start the fixed ChatGPT device-code flow.
    DeviceLogin,
}

const VERSION_ARGS: &[&str] = &["--version"];
const STATUS_ARGS: &[&str] = &[
    "-c",
    "forced_login_method=\"chatgpt\"",
    "-c",
    "cli_auth_credentials_store=\"file\"",
    "login",
    "status",
];
const DEVICE_LOGIN_ARGS: &[&str] = &[
    "-c",
    "forced_login_method=\"chatgpt\"",
    "-c",
    "cli_auth_credentials_store=\"file\"",
    "login",
    "--device-auth",
];

/// A non-extensible process policy for one pinned Codex CLI invocation.
///
/// Contract: `CU-AUTH-P0-02`. Callers receive fixed argv and the validated paths needed to build a
/// cleared-environment `std::process::Command`; no arbitrary argument or environment map exists.
pub struct CodexInvocation<'scope> {
    executable: &'scope Path,
    working_dir: &'scope Path,
    codex_home: &'scope Path,
    args: &'static [&'static str],
}

impl<'scope> CodexInvocation<'scope> {
    pub(crate) fn new(
        executable: &'scope Path,
        working_dir: &'scope Path,
        codex_home: &'scope Path,
        command: CodexCommand,
    ) -> Self {
        let args = match command {
            CodexCommand::Version => VERSION_ARGS,
            CodexCommand::LoginStatus => STATUS_ARGS,
            CodexCommand::DeviceLogin => DEVICE_LOGIN_ARGS,
        };

        Self {
            executable,
            working_dir,
            codex_home,
            args,
        }
    }

    /// Returns the validated absolute native executable path.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub fn executable(&self) -> &Path {
        self.executable
    }

    /// Returns the complete fixed argv excluding argv zero.
    ///
    /// Contract: `CU-AUTH-P0-02`. Browser and repository input cannot extend this slice.
    pub fn args(&self) -> &'static [&'static str] {
        self.args
    }

    /// Returns the validated non-repository working directory.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub fn working_dir(&self) -> &Path {
        self.working_dir
    }

    /// Returns the validated operator credential directory.
    ///
    /// Contract: `CU-AUTH-P0-02`. This projects the path only and never reads its contents.
    pub fn codex_home(&self) -> &Path {
        self.codex_home
    }

    /// Requires the process builder to call `env_clear` before setting `CODEX_HOME`.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub const fn clears_environment(&self) -> bool {
        true
    }
}

/// Revalidated trusted paths plus one non-extensible T003 Cloud argv policy.
pub(crate) struct CloudProcessInvocation<'scope, 'command> {
    executable: &'scope Path,
    working_dir: &'scope Path,
    codex_home: &'scope Path,
    command: &'command CloudInvocation,
}

impl<'scope, 'command> CloudProcessInvocation<'scope, 'command> {
    pub(crate) fn new(
        executable: &'scope Path,
        working_dir: &'scope Path,
        codex_home: &'scope Path,
        command: &'command CloudInvocation,
    ) -> Self {
        Self {
            executable,
            working_dir,
            codex_home,
            command,
        }
    }

    pub(crate) fn executable(&self) -> &Path {
        self.executable
    }

    pub(crate) fn working_dir(&self) -> &Path {
        self.working_dir
    }

    pub(crate) fn codex_home(&self) -> &Path {
        self.codex_home
    }

    pub(crate) fn args(&self) -> &[String] {
        self.command.args()
    }
}
