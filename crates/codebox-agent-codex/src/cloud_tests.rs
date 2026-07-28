use proptest::prelude::*;
use serde_json::{Value, json};

use crate::{
    CloudBranch, CloudCapture, CloudCursor, CloudEnvironmentId, CloudInvocation, CloudPrompt,
    CloudTaskId, CloudTaskStatus, decode_cloud_diff, decode_cloud_exec, decode_cloud_list,
    decode_cloud_status, decode_cloud_version,
};

const EXEC_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/exec.stdout");
const LIST_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/list.stdout.json");
const DIFF_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/diff.stdout");
const STATUS_PENDING_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/status.pending.stdout");
const STATUS_READY_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/status.ready.stdout");
const STATUS_APPLIED_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/status.applied.stdout");
const STATUS_ERROR_FIXTURE: &[u8] =
    include_bytes!("../../../docs/fixtures/codex-0.145.0/cloud/status.error.stdout");

fn capture(stdout: &[u8], stderr: &[u8], exit_code: Option<i32>) -> CloudCapture {
    CloudCapture::new(stdout.to_vec(), stderr.to_vec(), false, false, exit_code)
}

fn environment() -> CloudEnvironmentId {
    CloudEnvironmentId::try_new("env_synthetic").expect("valid environment")
}

fn branch() -> CloudBranch {
    CloudBranch::try_new("main").expect("valid branch")
}

fn prompt() -> CloudPrompt {
    CloudPrompt::try_new("Update docs; printf '%s' \"$HOME\"").expect("valid prompt")
}

fn task_id() -> CloudTaskId {
    CloudTaskId::try_new("task_i_abc123").expect("valid task ID")
}

#[test]
fn cloud_version_fixture_is_exact() {
    decode_cloud_version(&capture(b"codex-cli 0.145.0\n", b"", Some(0)))
        .expect("exact version fixture");

    for stdout in [
        b"codex-cli 0.145.1\n".as_slice(),
        b"codex-cli 0.145.0".as_slice(),
        b"codex-cli 0.145.0\r\n".as_slice(),
    ] {
        assert!(decode_cloud_version(&capture(stdout, b"", Some(0))).is_err());
    }
    assert!(decode_cloud_version(&capture(b"codex-cli 0.145.0\n", b"warning", Some(0))).is_err());
    assert!(decode_cloud_version(&capture(b"codex-cli 0.145.0\n", b"", Some(1))).is_err());
    assert!(
        decode_cloud_version(&CloudCapture::new(
            b"codex-cli 0.145.0\n".to_vec(),
            Vec::new(),
            true,
            false,
            Some(0),
        ))
        .is_err()
    );
}

#[test]
fn cloud_exec_fixture_is_exact() {
    let task = decode_cloud_exec(&capture(EXEC_FIXTURE, b"", Some(0))).expect("exec fixture");

    assert_eq!(
        task.as_str(),
        "https://chatgpt.com/codex/tasks/task_i_abc123"
    );
    assert_eq!(task.task_id().as_str(), "task_i_abc123");
}

#[test]
fn cloud_exec_rejects_origin_and_path_drift() {
    for stdout in [
        b"http://chatgpt.com/codex/tasks/task_i_abc123\n".as_slice(),
        b"https://example.com/codex/tasks/task_i_abc123\n".as_slice(),
        b"https://user@chatgpt.com/codex/tasks/task_i_abc123\n".as_slice(),
        b"https://chatgpt.com:443/codex/tasks/task_i_abc123\n".as_slice(),
        b"https://chatgpt.com/codex/task/task_i_abc123\n".as_slice(),
        b"https://chatgpt.com/codex/tasks/task_i_abc123?x=1\n".as_slice(),
        b"https://chatgpt.com/codex/tasks/task_i_abc123#fragment\n".as_slice(),
        b"https://chatgpt.com/codex/tasks/task_i_abc123\nextra\n".as_slice(),
        b"https://chatgpt.com/codex/tasks/task_i_abc123".as_slice(),
    ] {
        assert!(decode_cloud_exec(&capture(stdout, b"", Some(0))).is_err());
    }
}

#[test]
fn cloud_status_exit_mapping_is_exact() {
    for (fixture, exit_code, expected) in [
        (STATUS_PENDING_FIXTURE, 1, CloudTaskStatus::Pending),
        (STATUS_READY_FIXTURE, 0, CloudTaskStatus::Ready),
        (STATUS_APPLIED_FIXTURE, 1, CloudTaskStatus::Applied),
        (STATUS_ERROR_FIXTURE, 1, CloudTaskStatus::Error),
    ] {
        assert_eq!(
            decode_cloud_status(&capture(fixture, b"", Some(exit_code)))
                .expect("exact status fixture"),
            expected
        );
        assert!(
            decode_cloud_status(&capture(
                fixture,
                b"",
                Some(if exit_code == 0 { 1 } else { 0 })
            ))
            .is_err()
        );
    }

    let crlf = String::from_utf8(STATUS_READY_FIXTURE.to_vec())
        .expect("UTF-8 fixture")
        .replace('\n', "\r\n");
    assert_eq!(
        decode_cloud_status(&capture(crlf.as_bytes(), b"", Some(0))).expect("CRLF status"),
        CloudTaskStatus::Ready
    );
}

#[test]
fn cloud_status_rejects_control_and_template_drift() {
    for stdout in [
        b"\x1b[32m[READY]\x1b[0m title\nenv  \xe2\x80\xa2  1s ago\nno diff\n".as_slice(),
        b"[UNKNOWN] title\nenv  \xe2\x80\xa2  1s ago\nno diff\n".as_slice(),
        b"[READY] title\nenv  \xe2\x80\xa2  1s ago\n".as_slice(),
        b"[READY] title\n\nenv\n".as_slice(),
        b"[READY] title\nenv  \xe2\x80\xa2  1s ago\nno diff\nextra\n".as_slice(),
        b"[READY] title\nenv\t  \xe2\x80\xa2  1s ago\nno diff\n".as_slice(),
    ] {
        assert!(decode_cloud_status(&capture(stdout, b"", Some(0))).is_err());
    }
}

#[test]
fn cloud_list_fixture_is_exact() {
    let page = decode_cloud_list(&capture(LIST_FIXTURE, b"", Some(0))).expect("list fixture");

    assert_eq!(page.tasks().len(), 1);
    assert_eq!(
        page.cursor().expect("fixture cursor").as_str(),
        "cursor_synthetic_next"
    );
    let task = &page.tasks()[0];
    assert_eq!(task.id().as_str(), "task_i_abc123");
    assert_eq!(
        task.url().as_str(),
        "https://chatgpt.com/codex/tasks/task_i_abc123"
    );
    assert_eq!(task.status(), CloudTaskStatus::Ready);
    assert_eq!(task.title(), "Synthetic documentation update");
    assert_eq!(task.updated_at(), "2026-07-27T12:00:00Z");
    assert_eq!(task.environment_id(), Some("env_synthetic"));
    assert_eq!(task.environment_label(), Some("private-environment"));
    assert_eq!(task.files_changed(), 1);
    assert_eq!(task.lines_added(), 5);
    assert_eq!(task.lines_removed(), 1);
    assert!(!task.is_review());
    assert_eq!(task.attempts(), Some(1));
}

#[test]
fn cloud_list_rejects_schema_and_bound_violations() {
    let fixture: Value = serde_json::from_slice(LIST_FIXTURE).expect("JSON fixture");

    let mut unknown = fixture.clone();
    unknown["unexpected"] = json!(true);
    assert!(decode_cloud_list_json(unknown).is_err());

    let mut nested_unknown = fixture.clone();
    nested_unknown["tasks"][0]["summary"]["unexpected"] = json!(true);
    assert!(decode_cloud_list_json(nested_unknown).is_err());

    let mut duplicate = fixture.clone();
    let duplicate_row = duplicate["tasks"][0].clone();
    duplicate["tasks"]
        .as_array_mut()
        .expect("tasks array")
        .push(duplicate_row);
    assert!(decode_cloud_list_json(duplicate).is_err());

    let mut too_many = fixture.clone();
    let template = too_many["tasks"][0].clone();
    let rows = too_many["tasks"].as_array_mut().expect("tasks array");
    for index in 1..=20 {
        let mut row = template.clone();
        let id = format!("task_i_{index}");
        row["id"] = json!(id);
        row["url"] = json!(format!("https://chatgpt.com/codex/tasks/{id}"));
        rows.push(row);
    }
    assert!(decode_cloud_list_json(too_many).is_err());

    for (pointer, invalid) in [
        ("/tasks/0/attempt_total", json!(5)),
        ("/tasks/0/status", json!("queued")),
        ("/tasks/0/summary/files_changed", json!(1_000_001)),
        ("/tasks/0/summary/lines_added", json!(1_000_000_001_u64)),
        ("/tasks/0/summary/lines_removed", json!(1_000_000_001_u64)),
        ("/tasks/0/updated_at", json!("not-a-timestamp")),
        ("/tasks/0/title", json!("x".repeat(4_097))),
    ] {
        let mut value = fixture.clone();
        *value.pointer_mut(pointer).expect("fixture pointer") = invalid;
        assert!(decode_cloud_list_json(value).is_err(), "{pointer}");
    }

    for timestamp in [
        "202x-07-27T12:00:00Z",
        "2026-02-30T12:00:00Z",
        "2026-07-27T25:00:00Z",
        "2026-07-27T12:00:00+24:00",
    ] {
        let mut value = fixture.clone();
        value["tasks"][0]["updated_at"] = json!(timestamp);
        assert!(decode_cloud_list_json(value).is_err(), "{timestamp}");
    }

    let mut exact_limits = fixture.clone();
    exact_limits["tasks"][0]["attempt_total"] = json!(4);
    exact_limits["tasks"][0]["title"] = json!("x".repeat(4_096));
    exact_limits["tasks"][0]["summary"]["files_changed"] = json!(1_000_000);
    exact_limits["tasks"][0]["summary"]["lines_added"] = json!(1_000_000_000_u64);
    exact_limits["tasks"][0]["summary"]["lines_removed"] = json!(1_000_000_000_u64);
    assert!(decode_cloud_list_json(exact_limits).is_ok());

    for pointer in [
        "/cursor",
        "/tasks/0/attempt_total",
        "/tasks/0/environment_id",
        "/tasks/0/environment_label",
    ] {
        let mut missing = fixture.clone();
        remove_json_pointer(&mut missing, pointer);
        assert!(decode_cloud_list_json(missing).is_err(), "{pointer}");
    }
}

fn decode_cloud_list_json(
    value: Value,
) -> Result<crate::CloudTaskListPage, crate::CloudAdapterError> {
    let bytes = serde_json::to_vec(&value).expect("serialize test JSON");
    decode_cloud_list(&capture(&bytes, b"", Some(0)))
}

fn remove_json_pointer(value: &mut Value, pointer: &str) {
    let (parent, field) = pointer.rsplit_once('/').expect("test JSON pointer");
    value
        .pointer_mut(parent)
        .expect("test JSON parent")
        .as_object_mut()
        .expect("test JSON object")
        .remove(field);
}

type DecoderProbe = fn(&CloudCapture) -> bool;

#[test]
fn cloud_list_rejects_url_id_mismatch() {
    let mut fixture: Value = serde_json::from_slice(LIST_FIXTURE).expect("JSON fixture");
    fixture["tasks"][0]["url"] = json!("https://chatgpt.com/codex/tasks/task_i_different");

    assert!(decode_cloud_list_json(fixture).is_err());
}

#[test]
fn cloud_diff_is_bounded_and_untrusted() {
    let diff = decode_cloud_diff(&capture(DIFF_FIXTURE, b"", Some(0))).expect("diff fixture");
    assert_eq!(diff.as_str().as_bytes(), DIFF_FIXTURE);

    let empty = decode_cloud_diff(&capture(b"", b"", Some(0))).expect("empty diff");
    assert!(empty.as_str().is_empty());

    assert!(decode_cloud_diff(&capture(b"diff\0payload", b"", Some(0))).is_err());
    assert!(decode_cloud_diff(&capture(b"diff\rpayload", b"", Some(0))).is_err());
    assert!(decode_cloud_diff(&capture(&[0xff], b"", Some(0))).is_err());
    assert!(decode_cloud_diff(&capture(&vec![b'x'; 2 * 1024 * 1024], b"", Some(0))).is_ok());
    assert!(decode_cloud_diff(&capture(&vec![b'x'; 2 * 1024 * 1024 + 1], b"", Some(0))).is_err());
}

proptest! {
    #[test]
    fn cloud_decoders_are_chunk_partition_invariant(chunk_sizes in prop::collection::vec(0_usize..256, 0..64)) {
        let cases: &[(&[u8], i32, DecoderProbe)] = &[
            (b"codex-cli 0.145.0\n", 0, |capture| decode_cloud_version(capture).is_ok()),
            (EXEC_FIXTURE, 0, |capture| decode_cloud_exec(capture).is_ok()),
            (STATUS_READY_FIXTURE, 0, |capture| decode_cloud_status(capture).is_ok()),
            (LIST_FIXTURE, 0, |capture| decode_cloud_list(capture).is_ok()),
            (DIFF_FIXTURE, 0, |capture| decode_cloud_diff(capture).is_ok()),
        ];

        for (bytes, exit_code, decode) in cases {
            let reassembled = reassemble(bytes, &chunk_sizes);
            prop_assert_eq!(reassembled.as_slice(), *bytes);
            prop_assert!(decode(&capture(&reassembled, b"", Some(*exit_code))));
        }
    }
}

fn reassemble(bytes: &[u8], chunk_sizes: &[usize]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    for requested in chunk_sizes {
        if offset == bytes.len() {
            break;
        }
        let size = (*requested).max(1).min(bytes.len() - offset);
        output.extend_from_slice(&bytes[offset..offset + size]);
        offset += size;
    }
    output.extend_from_slice(&bytes[offset..]);
    output
}

#[test]
fn cloud_values_reject_unsafe_boundaries() {
    assert!(CloudEnvironmentId::try_new("").is_err());
    assert!(CloudEnvironmentId::try_new("-env").is_err());
    assert!(CloudEnvironmentId::try_new("env\nname").is_err());
    assert!(CloudEnvironmentId::try_new("x".repeat(256)).is_ok());
    assert!(CloudEnvironmentId::try_new("x".repeat(257)).is_err());
    assert_eq!(
        CloudEnvironmentId::try_new(" env ")
            .expect("trimmed environment")
            .as_str(),
        "env"
    );

    assert!(CloudBranch::try_new("").is_err());
    assert!(CloudBranch::try_new("-branch").is_err());
    assert!(CloudBranch::try_new("branch\0name").is_err());
    assert!(CloudBranch::try_new("x".repeat(255)).is_ok());
    assert!(CloudBranch::try_new("x".repeat(256)).is_err());

    assert!(CloudPrompt::try_new("").is_err());
    assert!(CloudPrompt::try_new(" \n\t ").is_err());
    assert!(CloudPrompt::try_new("-").is_err());
    assert!(CloudPrompt::try_new("prompt\rvalue").is_err());
    assert!(CloudPrompt::try_new("x".repeat(65_536)).is_ok());
    assert!(CloudPrompt::try_new("x".repeat(65_537)).is_err());
    assert!(CloudPrompt::try_new("line one\n\tline two").is_ok());

    assert!(CloudTaskId::try_new("task_").is_ok());
    assert!(CloudTaskId::try_new("other_abc").is_err());
    assert!(CloudTaskId::try_new("task_/abc").is_err());
    assert!(CloudTaskId::try_new(format!("task_{}", "x".repeat(123))).is_ok());
    assert!(CloudTaskId::try_new(format!("task_{}", "x".repeat(124))).is_err());

    assert!(CloudCursor::try_new("").is_err());
    assert!(CloudCursor::try_new("cursor\nvalue").is_err());
    assert!(CloudCursor::try_new("x".repeat(2_048)).is_ok());
    assert!(CloudCursor::try_new("x".repeat(2_049)).is_err());
    assert!(
        crate::CloudTaskUrl::try_new("https://chatgpt.com/codex/tasks/task_i_abc123?unexpected")
            .is_err()
    );
}

#[test]
fn cloud_prompt_cannot_inject_argv() {
    let prompt = prompt();
    let invocation = CloudInvocation::exec(&environment(), &branch(), &prompt);

    assert_eq!(
        invocation.args(),
        [
            "cloud",
            "exec",
            "--env=env_synthetic",
            "--attempts=1",
            "--branch=main",
            "--",
            "Update docs; printf '%s' \"$HOME\"",
        ]
    );
    assert_eq!(
        invocation
            .args()
            .iter()
            .filter(|argument| argument.as_str() == prompt.as_str())
            .count(),
        1
    );
}

#[test]
fn cloud_command_policy_has_no_apply_or_local_exec() {
    let injected_cursor = CloudCursor::try_new("-other-option value").expect("opaque cursor");
    let invocations = [
        CloudInvocation::version(),
        CloudInvocation::exec(&environment(), &branch(), &prompt()),
        CloudInvocation::status(&task_id()),
        CloudInvocation::list(&environment(), Some(&injected_cursor)),
        CloudInvocation::diff(&task_id()),
    ];

    assert_eq!(invocations[0].args(), ["--version"]);
    assert_eq!(invocations[2].args(), ["cloud", "status", "task_i_abc123"]);
    assert_eq!(
        invocations[3].args(),
        [
            "cloud",
            "list",
            "--env=env_synthetic",
            "--limit=20",
            "--json",
            "--cursor=-other-option value",
        ]
    );
    assert_eq!(
        invocations[4].args(),
        ["cloud", "diff", "--attempt=1", "task_i_abc123"]
    );

    for invocation in &invocations {
        assert!(!invocation.args().iter().any(|argument| argument == "apply"));
        assert!(!invocation.args().iter().any(|argument| argument == "login"));
        assert_ne!(invocation.args().first().map(String::as_str), Some("exec"));
        assert!(
            !invocation
                .args()
                .iter()
                .any(|argument| argument.contains('/'))
        );
    }
}

#[test]
fn cloud_decoder_errors_and_debug_are_redacted() {
    let canary = "T003_PROVIDER_OUTPUT_SECRET_CANARY";
    let error =
        decode_cloud_exec(&capture(canary.as_bytes(), b"", Some(0))).expect_err("invalid output");

    assert_eq!(error.field(), crate::CloudField::Stdout);
    assert_eq!(error.category(), crate::CloudErrorCategory::ProviderDrift);
    assert!(!error.to_string().contains(canary));
    assert!(!format!("{error:?}").contains(canary));
    assert!(!format!("{:?}", capture(canary.as_bytes(), b"", Some(0))).contains(canary));
    assert!(!format!("{:?}", CloudPrompt::try_new(canary).expect("prompt")).contains(canary));
    assert!(!format!("{:?}", CloudCursor::try_new(canary).expect("cursor")).contains(canary));

    let url = decode_cloud_exec(&capture(EXEC_FIXTURE, b"", Some(0))).expect("URL");
    assert!(!format!("{url:?}").contains("task_i_abc123"));

    let page = decode_cloud_list(&capture(LIST_FIXTURE, b"", Some(0))).expect("list");
    assert!(!format!("{page:?}").contains("Synthetic documentation update"));

    let diff = decode_cloud_diff(&capture(DIFF_FIXTURE, b"", Some(0))).expect("diff");
    assert!(!format!("{diff:?}").contains("Synthetic text"));

    let invocation = CloudInvocation::exec(
        &environment(),
        &branch(),
        &CloudPrompt::try_new(canary).expect("prompt"),
    );
    assert!(!format!("{invocation:?}").contains(canary));
}

#[test]
fn cloud_decoders_reject_missing_exit_status() {
    assert!(decode_cloud_version(&capture(b"codex-cli 0.145.0\n", b"", None)).is_err());
    assert!(decode_cloud_exec(&capture(EXEC_FIXTURE, b"", None)).is_err());
    assert!(decode_cloud_status(&capture(STATUS_READY_FIXTURE, b"", None)).is_err());
    assert!(decode_cloud_list(&capture(LIST_FIXTURE, b"", None)).is_err());
    assert!(decode_cloud_diff(&capture(DIFF_FIXTURE, b"", None)).is_err());
}

#[test]
fn regression_cloud_runner_never_executes_repository_code() {
    cloud_command_policy_has_no_apply_or_local_exec();
}

#[test]
fn regression_decoder_cannot_retry_unknown_cloud_submit() {
    let incomplete = capture(b"", b"", None);
    let first = decode_cloud_exec(&incomplete).expect_err("unknown submit outcome");
    let second = decode_cloud_exec(&incomplete).expect_err("deterministic decoder");

    assert_eq!(first, second);
    assert_eq!(
        CloudInvocation::exec(&environment(), &branch(), &prompt())
            .args()
            .iter()
            .filter(|argument| argument.as_str() == "--attempts=1")
            .count(),
        1
    );
}
