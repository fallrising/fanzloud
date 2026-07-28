use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::parser::PINNED_CODEX_VERSION_STDOUT;

const MAX_STANDARD_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENVIRONMENT_BYTES: usize = 256;
const MAX_BRANCH_BYTES: usize = 255;
const MAX_PROMPT_BYTES: usize = 65_536;
const MAX_TASK_ID_BYTES: usize = 128;
const MAX_CURSOR_BYTES: usize = 2_048;
const MAX_TITLE_BYTES: usize = 4_096;
const MAX_TASKS: usize = 20;
const MAX_FILES_CHANGED: u64 = 1_000_000;
const MAX_CHANGED_LINES: u64 = 1_000_000_000;
const TASK_URL_PREFIX: &str = "https://chatgpt.com/codex/tasks/";

/// The public field at which a Cloud contract failure occurred.
///
/// Contract: `CU-AGT-P0-01`. Field names are safe for logs; rejected values are never retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudField {
    Environment,
    Branch,
    Prompt,
    TaskId,
    TaskUrl,
    Cursor,
    Status,
    Title,
    UpdatedAt,
    EnvironmentId,
    EnvironmentLabel,
    FilesChanged,
    LinesAdded,
    LinesRemoved,
    Attempts,
    List,
    Diff,
    Stdout,
    Stderr,
    ExitStatus,
    Version,
}

impl fmt::Display for CloudField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Environment => "environment",
            Self::Branch => "branch",
            Self::Prompt => "prompt",
            Self::TaskId => "task ID",
            Self::TaskUrl => "task URL",
            Self::Cursor => "cursor",
            Self::Status => "status",
            Self::Title => "title",
            Self::UpdatedAt => "updated timestamp",
            Self::EnvironmentId => "task environment ID",
            Self::EnvironmentLabel => "task environment label",
            Self::FilesChanged => "files changed",
            Self::LinesAdded => "lines added",
            Self::LinesRemoved => "lines removed",
            Self::Attempts => "attempt count",
            Self::List => "task list",
            Self::Diff => "diff",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::ExitStatus => "exit status",
            Self::Version => "CLI version",
        })
    }
}

/// A redacted classification for one Cloud contract failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudErrorCategory {
    Empty,
    TooLong,
    ControlCharacter,
    LeadingHyphen,
    ReservedValue,
    InvalidFormat,
    InvalidUtf8,
    Overflow,
    Missing,
    Unexpected,
    Duplicate,
    LimitExceeded,
    ProviderDrift,
}

impl fmt::Display for CloudErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "is empty",
            Self::TooLong => "exceeds its size limit",
            Self::ControlCharacter => "contains a forbidden control character",
            Self::LeadingHyphen => "begins with a forbidden hyphen",
            Self::ReservedValue => "uses a reserved value",
            Self::InvalidFormat => "has an invalid format",
            Self::InvalidUtf8 => "is not valid UTF-8",
            Self::Overflow => "exceeded the capture limit",
            Self::Missing => "is missing",
            Self::Unexpected => "has an unexpected value",
            Self::Duplicate => "contains a duplicate",
            Self::LimitExceeded => "exceeds a numeric limit",
            Self::ProviderDrift => "does not match the pinned provider contract",
        })
    }
}

/// A typed, redacted failure at the pinned Codex Cloud contract boundary.
///
/// Contract: `CU-AGT-P0-01`. The error contains only a field and category. It never contains a
/// prompt, cursor, URL, title, diff, captured stream, or rejected input.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Codex Cloud {field} {category}")]
pub struct CloudAdapterError {
    field: CloudField,
    category: CloudErrorCategory,
}

impl CloudAdapterError {
    fn new(field: CloudField, category: CloudErrorCategory) -> Self {
        Self { field, category }
    }

    /// Returns the safe field classification.
    pub const fn field(&self) -> CloudField {
        self.field
    }

    /// Returns the safe failure category.
    pub const fn category(&self) -> CloudErrorCategory {
        self.category
    }
}

/// An administrator-configured Codex Cloud environment identifier.
///
/// Contract: `CU-AGT-P0-01`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CloudEnvironmentId(String);

impl CloudEnvironmentId {
    /// Validates and trims an administrator-configured environment identifier.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref().trim();
        validate_bounded_plain(value, CloudField::Environment, MAX_ENVIRONMENT_BYTES, true)?;
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated environment identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudEnvironmentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CloudEnvironmentId")
            .field(&self.0)
            .finish()
    }
}

/// An administrator-configured repository branch for a Codex Cloud environment.
///
/// Contract: `CU-AGT-P0-01`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CloudBranch(String);

impl CloudBranch {
    /// Validates and trims an administrator-configured branch.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref().trim();
        validate_bounded_plain(value, CloudField::Branch, MAX_BRANCH_BYTES, true)?;
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated branch.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudBranch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CloudBranch").field(&self.0).finish()
    }
}

/// A bounded browser prompt kept as exactly one Cloud exec argv item.
///
/// Contract: `CU-AGT-P0-01`. Debug output is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudPrompt(String);

impl CloudPrompt {
    /// Validates a prompt without normalizing or trimming its contents.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref();
        if value.is_empty() || value.chars().all(char::is_whitespace) {
            return Err(CloudAdapterError::new(
                CloudField::Prompt,
                CloudErrorCategory::Empty,
            ));
        }
        if value.len() > MAX_PROMPT_BYTES {
            return Err(CloudAdapterError::new(
                CloudField::Prompt,
                CloudErrorCategory::TooLong,
            ));
        }
        if value == "-" {
            return Err(CloudAdapterError::new(
                CloudField::Prompt,
                CloudErrorCategory::ReservedValue,
            ));
        }
        if value
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\n' && byte != b'\t')
        {
            return Err(CloudAdapterError::new(
                CloudField::Prompt,
                CloudErrorCategory::ControlCharacter,
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the exact validated prompt for the fixed argv vector.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CloudPrompt([REDACTED])")
    }
}

/// A provider-issued Codex Cloud task identifier.
///
/// Contract: `CU-AGT-P0-01`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CloudTaskId(String);

impl CloudTaskId {
    /// Validates the exact safe positional form accepted by the pinned CLI.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(CloudAdapterError::new(
                CloudField::TaskId,
                CloudErrorCategory::Empty,
            ));
        }
        if value.len() > MAX_TASK_ID_BYTES {
            return Err(CloudAdapterError::new(
                CloudField::TaskId,
                CloudErrorCategory::TooLong,
            ));
        }
        if !value.starts_with("task_")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(CloudAdapterError::new(
                CloudField::TaskId,
                CloudErrorCategory::InvalidFormat,
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated task identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudTaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CloudTaskId").field(&self.0).finish()
    }
}

/// The exact pinned browser URL for one Codex Cloud task.
///
/// Contract: `CU-AGT-P0-01`. Debug output is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudTaskUrl {
    value: String,
    task_id: CloudTaskId,
}

impl CloudTaskUrl {
    /// Validates the exact pinned HTTPS origin, path, and embedded task ID.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref();
        let Some(task_id) = value.strip_prefix(TASK_URL_PREFIX) else {
            return Err(CloudAdapterError::new(
                CloudField::TaskUrl,
                CloudErrorCategory::InvalidFormat,
            ));
        };
        let task_id = CloudTaskId::try_new(task_id).map_err(|_| {
            CloudAdapterError::new(CloudField::TaskUrl, CloudErrorCategory::InvalidFormat)
        })?;
        Ok(Self {
            value: value.to_owned(),
            task_id,
        })
    }

    /// Returns the exact validated HTTPS URL.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns the validated task identifier embedded in the URL.
    pub const fn task_id(&self) -> &CloudTaskId {
        &self.task_id
    }
}

impl fmt::Debug for CloudTaskUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CloudTaskUrl([REDACTED])")
    }
}

/// A provider-issued opaque Cloud list cursor.
///
/// Contract: `CU-AGT-P0-01`. Debug output is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudCursor(String);

impl CloudCursor {
    /// Validates a bounded opaque provider cursor.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, CloudAdapterError> {
        let value = value.as_ref();
        validate_bounded_plain(value, CloudField::Cursor, MAX_CURSOR_BYTES, false)?;
        Ok(Self(value.to_owned()))
    }

    /// Returns the cursor for one fixed `--cursor=<value>` argv item.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CloudCursor([REDACTED])")
    }
}

/// A normalized pinned Codex Cloud task status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudTaskStatus {
    Pending,
    Ready,
    Applied,
    Error,
}

/// One bounded task row from the exact pinned Cloud list schema.
///
/// Contract: `CU-AGT-P0-01`. Debug output omits provider display content.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudTaskSummary {
    id: CloudTaskId,
    url: CloudTaskUrl,
    status: CloudTaskStatus,
    title: String,
    updated_at: String,
    environment_id: Option<String>,
    environment_label: Option<String>,
    files_changed: u64,
    lines_added: u64,
    lines_removed: u64,
    is_review: bool,
    attempts: Option<u8>,
}

impl CloudTaskSummary {
    /// Returns the validated row task identifier.
    pub const fn id(&self) -> &CloudTaskId {
        &self.id
    }

    /// Returns the validated row task URL.
    pub const fn url(&self) -> &CloudTaskUrl {
        &self.url
    }

    /// Returns the normalized provider status.
    pub const fn status(&self) -> CloudTaskStatus {
        self.status
    }

    /// Returns the bounded provider display title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the validated RFC3339 update timestamp.
    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }

    /// Returns the optional bounded provider environment ID.
    pub fn environment_id(&self) -> Option<&str> {
        self.environment_id.as_deref()
    }

    /// Returns the optional bounded provider environment label.
    pub fn environment_label(&self) -> Option<&str> {
        self.environment_label.as_deref()
    }

    /// Returns the bounded changed-file count.
    pub const fn files_changed(&self) -> u64 {
        self.files_changed
    }

    /// Returns the bounded added-line count.
    pub const fn lines_added(&self) -> u64 {
        self.lines_added
    }

    /// Returns the bounded removed-line count.
    pub const fn lines_removed(&self) -> u64 {
        self.lines_removed
    }

    /// Returns whether the pinned provider row classifies this task as a review.
    pub const fn is_review(&self) -> bool {
        self.is_review
    }

    /// Returns the optional pinned attempt count.
    pub const fn attempts(&self) -> Option<u8> {
        self.attempts
    }
}

impl fmt::Debug for CloudTaskSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CloudTaskSummary([REDACTED])")
    }
}

/// One exact, bounded page from `codex cloud list --json`.
///
/// Contract: `CU-AGT-P0-01`. Debug output omits task display fields and the cursor.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudTaskListPage {
    tasks: Vec<CloudTaskSummary>,
    cursor: Option<CloudCursor>,
}

impl CloudTaskListPage {
    /// Returns the at-most-20 validated task summaries.
    pub fn tasks(&self) -> &[CloudTaskSummary] {
        &self.tasks
    }

    /// Returns the optional opaque pagination cursor.
    pub const fn cursor(&self) -> Option<&CloudCursor> {
        self.cursor.as_ref()
    }
}

impl fmt::Debug for CloudTaskListPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudTaskListPage")
            .field("task_count", &self.tasks.len())
            .field("cursor", &"[REDACTED]")
            .finish()
    }
}

/// A bounded, untrusted raw unified-diff display value.
///
/// Contract: `CU-AGT-P0-01`. This type carries no validation or application authority for paths or
/// patch contents. Debug output is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudDiff(String);

impl CloudDiff {
    /// Returns the raw validated display text without interpreting or applying it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudDiff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CloudDiff([REDACTED])")
    }
}

/// A completed bounded stdout/stderr capture supplied by a future process supervisor.
///
/// Contract: `CU-AGT-P0-01`. The container owns no live stream or process. Debug output never
/// includes captured bytes.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct CloudCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_overflow: bool,
    stderr_overflow: bool,
    exit_code: Option<i32>,
}

impl CloudCapture {
    /// Constructs a completed capture without interpreting its untrusted contents.
    pub fn new(
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_overflow: bool,
        stderr_overflow: bool,
        exit_code: Option<i32>,
    ) -> Self {
        Self {
            stdout,
            stderr,
            stdout_overflow,
            stderr_overflow,
            exit_code,
        }
    }
}

impl fmt::Debug for CloudCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudCapture")
            .field("stdout", &"[REDACTED]")
            .field("stderr", &"[REDACTED]")
            .field("stdout_overflow", &self.stdout_overflow)
            .field("stderr_overflow", &self.stderr_overflow)
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloudInvocationKind {
    Version,
    Exec,
    Status,
    List,
    Diff,
}

/// A non-extensible argv policy for the pinned Codex Cloud CLI surface.
///
/// Contract: `CU-AGT-P0-01`. This value contains no executable, environment, working directory,
/// process builder, retry, `cloud apply`, local `codex exec`, or execution method.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudInvocation {
    kind: CloudInvocationKind,
    args: Vec<String>,
}

impl CloudInvocation {
    /// Constructs the exact pinned CLI version argv.
    pub fn version() -> Self {
        Self {
            kind: CloudInvocationKind::Version,
            args: vec!["--version".to_owned()],
        }
    }

    /// Constructs the exact one-attempt Cloud submission argv.
    pub fn exec(
        environment: &CloudEnvironmentId,
        branch: &CloudBranch,
        prompt: &CloudPrompt,
    ) -> Self {
        Self {
            kind: CloudInvocationKind::Exec,
            args: vec![
                "cloud".to_owned(),
                "exec".to_owned(),
                format!("--env={}", environment.as_str()),
                "--attempts=1".to_owned(),
                format!("--branch={}", branch.as_str()),
                "--".to_owned(),
                prompt.as_str().to_owned(),
            ],
        }
    }

    /// Constructs the exact Cloud status argv for a validated task ID.
    pub fn status(task_id: &CloudTaskId) -> Self {
        Self {
            kind: CloudInvocationKind::Status,
            args: vec![
                "cloud".to_owned(),
                "status".to_owned(),
                task_id.as_str().to_owned(),
            ],
        }
    }

    /// Constructs the exact bounded Cloud list argv and optional one-item cursor option.
    pub fn list(environment: &CloudEnvironmentId, cursor: Option<&CloudCursor>) -> Self {
        let mut args = vec![
            "cloud".to_owned(),
            "list".to_owned(),
            format!("--env={}", environment.as_str()),
            "--limit=20".to_owned(),
            "--json".to_owned(),
        ];
        if let Some(cursor) = cursor {
            args.push(format!("--cursor={}", cursor.as_str()));
        }
        Self {
            kind: CloudInvocationKind::List,
            args,
        }
    }

    /// Constructs the exact first-attempt Cloud diff argv for a validated task ID.
    pub fn diff(task_id: &CloudTaskId) -> Self {
        Self {
            kind: CloudInvocationKind::Diff,
            args: vec![
                "cloud".to_owned(),
                "diff".to_owned(),
                "--attempt=1".to_owned(),
                task_id.as_str().to_owned(),
            ],
        }
    }

    /// Returns the complete ordered argv excluding argv zero.
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

impl fmt::Debug for CloudInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudInvocation")
            .field("kind", &self.kind)
            .field("args", &"[REDACTED]")
            .finish()
    }
}

/// Verifies the exact CLI version fixture shared with the accepted login broker.
pub fn decode_cloud_version(capture: &CloudCapture) -> Result<(), CloudAdapterError> {
    validate_standard_capture(capture)?;
    require_exit(capture, 0)?;
    if capture.stdout == PINNED_CODEX_VERSION_STDOUT.as_bytes() {
        Ok(())
    } else {
        Err(CloudAdapterError::new(
            CloudField::Version,
            CloudErrorCategory::ProviderDrift,
        ))
    }
}

/// Decodes the exact successful Cloud exec URL and embedded task ID.
pub fn decode_cloud_exec(capture: &CloudCapture) -> Result<CloudTaskUrl, CloudAdapterError> {
    validate_standard_capture(capture)?;
    require_exit(capture, 0)?;
    let stdout = capture_stdout(capture)?;
    let Some(url) = stdout.strip_suffix('\n') else {
        return Err(CloudAdapterError::new(
            CloudField::Stdout,
            CloudErrorCategory::ProviderDrift,
        ));
    };
    if url.is_empty() || url.contains(['\r', '\n']) {
        return Err(CloudAdapterError::new(
            CloudField::Stdout,
            CloudErrorCategory::ProviderDrift,
        ));
    }
    CloudTaskUrl::try_new(url)
}

/// Decodes the exact three-line status shape and pinned status/exit mapping.
pub fn decode_cloud_status(capture: &CloudCapture) -> Result<CloudTaskStatus, CloudAdapterError> {
    validate_standard_capture(capture)?;
    let exit_code = capture.exit_code.ok_or_else(|| {
        CloudAdapterError::new(CloudField::ExitStatus, CloudErrorCategory::Missing)
    })?;
    let stdout = normalize_status_output(capture_stdout(capture)?)?;
    let Some(without_final_lf) = stdout.strip_suffix('\n') else {
        return Err(provider_drift(CloudField::Status));
    };
    let lines: Vec<&str> = without_final_lf.split('\n').collect();
    if lines.len() != 3 || lines.iter().any(|line| line.is_empty()) {
        return Err(provider_drift(CloudField::Status));
    }

    let (status, title) = parse_status_label(lines[0])?;
    validate_display_field(title, CloudField::Title, MAX_TITLE_BYTES)?;
    let Some((environment, age)) = lines[1].split_once("  •  ") else {
        return Err(provider_drift(CloudField::Status));
    };
    if age.contains("  •  ") {
        return Err(provider_drift(CloudField::Status));
    }
    validate_display_field(
        environment,
        CloudField::EnvironmentLabel,
        MAX_ENVIRONMENT_BYTES,
    )?;
    validate_display_field(age, CloudField::Status, MAX_ENVIRONMENT_BYTES)?;
    validate_display_field(lines[2], CloudField::Status, MAX_ENVIRONMENT_BYTES)?;

    let expected_exit = if status == CloudTaskStatus::Ready {
        0
    } else {
        1
    };
    if exit_code != expected_exit {
        return Err(provider_drift(CloudField::ExitStatus));
    }
    Ok(status)
}

/// Decodes the exact pinned JSON task-list schema.
pub fn decode_cloud_list(capture: &CloudCapture) -> Result<CloudTaskListPage, CloudAdapterError> {
    validate_standard_capture(capture)?;
    require_exit(capture, 0)?;
    let raw: RawTaskListPage =
        serde_json::from_slice(&capture.stdout).map_err(|_| provider_drift(CloudField::List))?;
    if raw.tasks.len() > MAX_TASKS {
        return Err(CloudAdapterError::new(
            CloudField::List,
            CloudErrorCategory::LimitExceeded,
        ));
    }

    let cursor = required_optional_string(raw.cursor, CloudField::Cursor)?
        .map(CloudCursor::try_new)
        .transpose()?;
    let mut ids = HashSet::with_capacity(raw.tasks.len());
    let mut tasks = Vec::with_capacity(raw.tasks.len());
    for row in raw.tasks {
        let id = CloudTaskId::try_new(row.id)?;
        if !ids.insert(id.clone()) {
            return Err(CloudAdapterError::new(
                CloudField::TaskId,
                CloudErrorCategory::Duplicate,
            ));
        }
        let url = CloudTaskUrl::try_new(row.url)?;
        if url.task_id() != &id {
            return Err(provider_drift(CloudField::TaskUrl));
        }
        let status = parse_list_status(&row.status)?;
        validate_list_text(&row.title, CloudField::Title, MAX_TITLE_BYTES)?;
        if !valid_rfc3339(&row.updated_at) {
            return Err(CloudAdapterError::new(
                CloudField::UpdatedAt,
                CloudErrorCategory::InvalidFormat,
            ));
        }
        let environment_id = validate_optional_list_text(
            required_optional_string(row.environment_id, CloudField::EnvironmentId)?,
            CloudField::EnvironmentId,
            MAX_ENVIRONMENT_BYTES,
        )?;
        let environment_label = validate_optional_list_text(
            required_optional_string(row.environment_label, CloudField::EnvironmentLabel)?,
            CloudField::EnvironmentLabel,
            MAX_ENVIRONMENT_BYTES,
        )?;
        validate_numeric_limit(
            row.summary.files_changed,
            MAX_FILES_CHANGED,
            CloudField::FilesChanged,
        )?;
        validate_numeric_limit(
            row.summary.lines_added,
            MAX_CHANGED_LINES,
            CloudField::LinesAdded,
        )?;
        validate_numeric_limit(
            row.summary.lines_removed,
            MAX_CHANGED_LINES,
            CloudField::LinesRemoved,
        )?;
        let attempts = required_optional_u8(row.attempt_total, CloudField::Attempts)?;
        if attempts.is_some_and(|attempts| !(1..=4).contains(&attempts)) {
            return Err(CloudAdapterError::new(
                CloudField::Attempts,
                CloudErrorCategory::LimitExceeded,
            ));
        }

        tasks.push(CloudTaskSummary {
            id,
            url,
            status,
            title: row.title,
            updated_at: row.updated_at,
            environment_id,
            environment_label,
            files_changed: row.summary.files_changed,
            lines_added: row.summary.lines_added,
            lines_removed: row.summary.lines_removed,
            is_review: row.is_review,
            attempts,
        });
    }

    Ok(CloudTaskListPage { tasks, cursor })
}

/// Decodes a bounded raw diff without interpreting, normalizing, persisting, or applying it.
pub fn decode_cloud_diff(capture: &CloudCapture) -> Result<CloudDiff, CloudAdapterError> {
    validate_capture(capture, MAX_DIFF_BYTES)?;
    require_exit(capture, 0)?;
    let value = std::str::from_utf8(&capture.stdout)
        .map_err(|_| CloudAdapterError::new(CloudField::Diff, CloudErrorCategory::InvalidUtf8))?;
    if value
        .bytes()
        .any(|byte| byte.is_ascii_control() && byte != b'\n' && byte != b'\t')
    {
        return Err(CloudAdapterError::new(
            CloudField::Diff,
            CloudErrorCategory::ControlCharacter,
        ));
    }
    Ok(CloudDiff(value.to_owned()))
}

fn validate_bounded_plain(
    value: &str,
    field: CloudField,
    max_bytes: usize,
    reject_leading_hyphen: bool,
) -> Result<(), CloudAdapterError> {
    if value.is_empty() {
        return Err(CloudAdapterError::new(field, CloudErrorCategory::Empty));
    }
    if value.len() > max_bytes {
        return Err(CloudAdapterError::new(field, CloudErrorCategory::TooLong));
    }
    if reject_leading_hyphen && value.starts_with('-') {
        return Err(CloudAdapterError::new(
            field,
            CloudErrorCategory::LeadingHyphen,
        ));
    }
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(CloudAdapterError::new(
            field,
            CloudErrorCategory::ControlCharacter,
        ));
    }
    Ok(())
}

fn validate_standard_capture(capture: &CloudCapture) -> Result<(), CloudAdapterError> {
    validate_capture(capture, MAX_STANDARD_CAPTURE_BYTES)
}

fn validate_capture(
    capture: &CloudCapture,
    max_stdout_bytes: usize,
) -> Result<(), CloudAdapterError> {
    if capture.stdout_overflow {
        return Err(CloudAdapterError::new(
            CloudField::Stdout,
            CloudErrorCategory::Overflow,
        ));
    }
    if capture.stderr_overflow {
        return Err(CloudAdapterError::new(
            CloudField::Stderr,
            CloudErrorCategory::Overflow,
        ));
    }
    if capture.stdout.len() > max_stdout_bytes {
        return Err(CloudAdapterError::new(
            CloudField::Stdout,
            CloudErrorCategory::TooLong,
        ));
    }
    if capture.stderr.len() > MAX_STANDARD_CAPTURE_BYTES {
        return Err(CloudAdapterError::new(
            CloudField::Stderr,
            CloudErrorCategory::TooLong,
        ));
    }
    if capture.exit_code.is_none() {
        return Err(CloudAdapterError::new(
            CloudField::ExitStatus,
            CloudErrorCategory::Missing,
        ));
    }
    if !capture.stderr.is_empty() {
        return Err(CloudAdapterError::new(
            CloudField::Stderr,
            CloudErrorCategory::Unexpected,
        ));
    }
    Ok(())
}

fn require_exit(capture: &CloudCapture, expected: i32) -> Result<(), CloudAdapterError> {
    if capture.exit_code == Some(expected) {
        Ok(())
    } else {
        Err(CloudAdapterError::new(
            CloudField::ExitStatus,
            CloudErrorCategory::Unexpected,
        ))
    }
}

fn capture_stdout(capture: &CloudCapture) -> Result<&str, CloudAdapterError> {
    std::str::from_utf8(&capture.stdout)
        .map_err(|_| CloudAdapterError::new(CloudField::Stdout, CloudErrorCategory::InvalidUtf8))
}

fn normalize_status_output(value: &str) -> Result<String, CloudAdapterError> {
    let normalized = value.replace("\r\n", "\n");
    if normalized.contains('\r')
        || normalized
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(CloudAdapterError::new(
            CloudField::Status,
            CloudErrorCategory::ControlCharacter,
        ));
    }
    Ok(normalized)
}

fn parse_status_label(line: &str) -> Result<(CloudTaskStatus, &str), CloudAdapterError> {
    for (prefix, status) in [
        ("[PENDING] ", CloudTaskStatus::Pending),
        ("[READY] ", CloudTaskStatus::Ready),
        ("[APPLIED] ", CloudTaskStatus::Applied),
        ("[ERROR] ", CloudTaskStatus::Error),
    ] {
        if let Some(display) = line.strip_prefix(prefix) {
            return Ok((status, display));
        }
    }
    Err(provider_drift(CloudField::Status))
}

fn validate_display_field(
    value: &str,
    field: CloudField,
    max_bytes: usize,
) -> Result<(), CloudAdapterError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(provider_drift(field));
    }
    Ok(())
}

fn validate_list_text(
    value: &str,
    field: CloudField,
    max_bytes: usize,
) -> Result<(), CloudAdapterError> {
    if value.len() > max_bytes {
        return Err(CloudAdapterError::new(field, CloudErrorCategory::TooLong));
    }
    if value.chars().any(char::is_control) {
        return Err(CloudAdapterError::new(
            field,
            CloudErrorCategory::ControlCharacter,
        ));
    }
    Ok(())
}

fn validate_optional_list_text(
    value: Option<String>,
    field: CloudField,
    max_bytes: usize,
) -> Result<Option<String>, CloudAdapterError> {
    if let Some(value) = value {
        validate_list_text(&value, field, max_bytes)?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

fn required_optional_string(
    value: Value,
    field: CloudField,
) -> Result<Option<String>, CloudAdapterError> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(provider_drift(field)),
    }
}

fn required_optional_u8(value: Value, field: CloudField) -> Result<Option<u8>, CloudAdapterError> {
    match value {
        Value::Null => Ok(None),
        Value::Number(value) => value
            .as_u64()
            .and_then(|value| u8::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| provider_drift(field)),
        _ => Err(provider_drift(field)),
    }
}

fn validate_numeric_limit(
    value: u64,
    limit: u64,
    field: CloudField,
) -> Result<(), CloudAdapterError> {
    if value <= limit {
        Ok(())
    } else {
        Err(CloudAdapterError::new(
            field,
            CloudErrorCategory::LimitExceeded,
        ))
    }
}

fn parse_list_status(value: &str) -> Result<CloudTaskStatus, CloudAdapterError> {
    match value {
        "pending" => Ok(CloudTaskStatus::Pending),
        "ready" => Ok(CloudTaskStatus::Ready),
        "applied" => Ok(CloudTaskStatus::Applied),
        "error" => Ok(CloudTaskStatus::Error),
        _ => Err(provider_drift(CloudField::Status)),
    }
}

fn provider_drift(field: CloudField) -> CloudAdapterError {
    CloudAdapterError::new(field, CloudErrorCategory::ProviderDrift)
}

fn valid_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let Some(year) = parse_digits(bytes, 0, 4) else {
        return false;
    };
    let Some(month) = parse_digits(bytes, 5, 2) else {
        return false;
    };
    let Some(day) = parse_digits(bytes, 8, 2) else {
        return false;
    };
    let Some(hour) = parse_digits(bytes, 11, 2) else {
        return false;
    };
    let Some(minute) = parse_digits(bytes, 14, 2) else {
        return false;
    };
    let Some(second) = parse_digits(bytes, 17, 2) else {
        return false;
    };
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }

    let mut timezone_start = 19;
    if bytes.get(timezone_start) == Some(&b'.') {
        timezone_start += 1;
        let fraction_start = timezone_start;
        while matches!(bytes.get(timezone_start), Some(byte) if byte.is_ascii_digit()) {
            timezone_start += 1;
        }
        if timezone_start == fraction_start {
            return false;
        }
    }

    match bytes.get(timezone_start..) {
        Some([b'Z' | b'z']) => true,
        Some(
            [
                sign @ (b'+' | b'-'),
                hour_a,
                hour_b,
                b':',
                minute_a,
                minute_b,
            ],
        ) => {
            let _ = sign;
            let offset_hour = digit_pair(*hour_a, *hour_b);
            let offset_minute = digit_pair(*minute_a, *minute_b);
            matches!(
                (offset_hour, offset_minute),
                (Some(hour), Some(minute)) if hour <= 23 && minute <= 59
            )
        }
        _ => false,
    }
}

fn parse_digits(bytes: &[u8], start: usize, len: usize) -> Option<u32> {
    bytes
        .get(start..start + len)?
        .iter()
        .try_fold(0_u32, |value, byte| {
            if byte.is_ascii_digit() {
                Some(value * 10 + u32::from(*byte - b'0'))
            } else {
                None
            }
        })
}

fn digit_pair(first: u8, second: u8) -> Option<u32> {
    if first.is_ascii_digit() && second.is_ascii_digit() {
        Some(u32::from(first - b'0') * 10 + u32::from(second - b'0'))
    } else {
        None
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTaskListPage {
    cursor: Value,
    tasks: Vec<RawTaskSummary>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTaskSummary {
    attempt_total: Value,
    environment_id: Value,
    environment_label: Value,
    id: String,
    is_review: bool,
    status: String,
    summary: RawDiffSummary,
    title: String,
    updated_at: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDiffSummary {
    files_changed: u64,
    lines_added: u64,
    lines_removed: u64,
}
