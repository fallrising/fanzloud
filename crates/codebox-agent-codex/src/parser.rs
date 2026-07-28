use crate::{LoginBrokerError, VerificationCode};

pub(crate) const MAX_CAPTURE_BYTES: usize = 16 * 1024;
const DEVICE_CODE_PLACEHOLDER: &str = "A1B2-3456C";
const DEVICE_PROMPT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/device-login.stdout");
const STATUS_CHATGPT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/login-status.chatgpt.stderr");
const STATUS_LOGGED_OUT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/login-status.logged-out.stderr");
const LOGIN_SUCCESS_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/device-login.success.stderr");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PinnedStatus {
    LoggedOut,
    LoggedIn,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CapturedOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_overflow: bool,
    pub stderr_overflow: bool,
    pub exit_code: Option<i32>,
}

pub(crate) fn parse_version(output: &CapturedOutput) -> Result<(), LoginBrokerError> {
    reject_overflow(output)?;
    if output.exit_code != Some(0)
        || normalize_plain_stream(&output.stdout)? != "codex-cli 0.145.0\n"
        || !output.stderr.is_empty()
    {
        return Err(LoginBrokerError::VersionMismatch);
    }
    Ok(())
}

pub(crate) fn parse_status(output: &CapturedOutput) -> Result<PinnedStatus, LoginBrokerError> {
    reject_overflow(output)?;
    if !output.stdout.is_empty() {
        return Err(LoginBrokerError::ProviderOutputInvalid);
    }
    let stderr = normalize_plain_stream(&output.stderr)?;

    match (output.exit_code, stderr.as_str()) {
        (Some(0), STATUS_CHATGPT_FIXTURE) => Ok(PinnedStatus::LoggedIn),
        (Some(1), STATUS_LOGGED_OUT_FIXTURE) => Ok(PinnedStatus::LoggedOut),
        (Some(1), _) | (None, _) => Err(LoginBrokerError::StatusUnavailable),
        _ => Err(LoginBrokerError::ProviderOutputInvalid),
    }
}

pub(crate) fn parse_device_prompt(
    stdout: &[u8],
    overflow: bool,
) -> Result<VerificationCode, PromptParseError> {
    if overflow {
        return Err(PromptParseError::Terminal(
            LoginBrokerError::OutputLimitExceeded,
        ));
    }

    let normalized = normalize_ansi_stream(stdout)?;
    if normalized.len() < DEVICE_PROMPT_FIXTURE.len() {
        return Err(PromptParseError::Incomplete);
    }
    if normalized.len() != DEVICE_PROMPT_FIXTURE.len() {
        return Err(PromptParseError::Terminal(
            LoginBrokerError::ProviderOutputInvalid,
        ));
    }

    let Some(placeholder_start) = DEVICE_PROMPT_FIXTURE.find(DEVICE_CODE_PLACEHOLDER) else {
        return Err(PromptParseError::Terminal(
            LoginBrokerError::ProviderOutputInvalid,
        ));
    };
    let placeholder_end = placeholder_start + DEVICE_CODE_PLACEHOLDER.len();
    if normalized[..placeholder_start] != DEVICE_PROMPT_FIXTURE[..placeholder_start]
        || normalized[placeholder_end..] != DEVICE_PROMPT_FIXTURE[placeholder_end..]
    {
        return Err(PromptParseError::Terminal(
            LoginBrokerError::ProviderOutputInvalid,
        ));
    }

    let code = &normalized[placeholder_start..placeholder_end];
    if !valid_device_code(code.as_bytes()) {
        return Err(PromptParseError::Terminal(
            LoginBrokerError::ProviderOutputInvalid,
        ));
    }

    Ok(VerificationCode::from_validated(code.to_owned()))
}

pub(crate) fn parse_login_completion(output: &CapturedOutput) -> Result<(), LoginBrokerError> {
    reject_overflow(output)?;
    parse_device_prompt(&output.stdout, false)
        .map_err(|_| LoginBrokerError::ProviderOutputInvalid)?;
    if output.exit_code == Some(0)
        && normalize_plain_stream(&output.stderr)? == LOGIN_SUCCESS_FIXTURE
    {
        Ok(())
    } else if output.exit_code.is_some() {
        Err(LoginBrokerError::LoginFailed)
    } else {
        Err(LoginBrokerError::OutcomeUnknown)
    }
}

#[derive(Debug)]
pub(crate) enum PromptParseError {
    Incomplete,
    Terminal(LoginBrokerError),
}

fn reject_overflow(output: &CapturedOutput) -> Result<(), LoginBrokerError> {
    if output.stdout_overflow || output.stderr_overflow {
        Err(LoginBrokerError::OutputLimitExceeded)
    } else {
        Ok(())
    }
}

fn normalize_plain_stream(bytes: &[u8]) -> Result<String, LoginBrokerError> {
    let value = std::str::from_utf8(bytes).map_err(|_| LoginBrokerError::ProviderOutputInvalid)?;
    normalize_line_endings(value).map_err(|_| LoginBrokerError::ProviderOutputInvalid)
}

fn normalize_ansi_stream(bytes: &[u8]) -> Result<String, PromptParseError> {
    let mut stripped = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != 0x1b {
            stripped.push(bytes[index]);
            index += 1;
            continue;
        }

        if index + 1 >= bytes.len() {
            return Err(PromptParseError::Incomplete);
        }
        if bytes[index + 1] != b'[' {
            return Err(PromptParseError::Terminal(
                LoginBrokerError::ProviderOutputInvalid,
            ));
        }

        let mut end = index + 2;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b';') {
            end += 1;
        }
        if end >= bytes.len() {
            return Err(PromptParseError::Incomplete);
        }
        if bytes[end] != b'm' {
            return Err(PromptParseError::Terminal(
                LoginBrokerError::ProviderOutputInvalid,
            ));
        }
        index = end + 1;
    }

    let value = std::str::from_utf8(&stripped)
        .map_err(|_| PromptParseError::Terminal(LoginBrokerError::ProviderOutputInvalid))?;
    normalize_line_endings(value)
        .map_err(|_| PromptParseError::Terminal(LoginBrokerError::ProviderOutputInvalid))
}

fn normalize_line_endings(value: &str) -> Result<String, ()> {
    let normalized = value.replace("\r\n", "\n");
    if normalized.contains('\r')
        || normalized
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\n')
    {
        return Err(());
    }
    Ok(normalized)
}

fn valid_device_code(code: &[u8]) -> bool {
    code.len() == 10
        && code[4] == b'-'
        && code
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}
