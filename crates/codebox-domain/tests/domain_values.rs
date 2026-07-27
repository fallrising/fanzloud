use codebox_domain::{
    ApprovalId, ArtifactId, CommandId, DomainError, EventSeq, EventSeqError, IdError,
    MAX_WORKSPACE_PATH_BYTES, SandboxId, SessionId, ToolCallId, TurnId, WorkspacePath,
    WorkspacePathError,
};
use uuid::Uuid;

#[test]
fn ids_are_non_nil() {
    macro_rules! assert_non_nil {
        ($($id:ty),+ $(,)?) => {
            $(assert!(!<$id>::new().as_uuid().is_nil());)+
        };
    }

    assert_non_nil!(
        SessionId, TurnId, ToolCallId, ApprovalId, SandboxId, ArtifactId, CommandId,
    );
}

#[test]
fn nil_ids_are_rejected() {
    macro_rules! assert_nil_rejected {
        ($($id:ty),+ $(,)?) => {
            $(
                assert_eq!(
                    <$id>::try_from_uuid(Uuid::nil()),
                    Err(IdError::Nil { id_type: stringify!($id) }),
                );
            )+
        };
    }

    assert_nil_rejected!(
        SessionId, TurnId, ToolCallId, ApprovalId, SandboxId, ArtifactId, CommandId,
    );
}

#[test]
fn ids_round_trip_through_serde() {
    macro_rules! assert_round_trip {
        ($($id:ty),+ $(,)?) => {
            $(
                let original = <$id>::new();
                let encoded = serde_json::to_string(&original).expect("ID serialization");
                let decoded: $id = serde_json::from_str(&encoded).expect("ID deserialization");
                assert_eq!(decoded, original);
            )+
        };
    }

    assert_round_trip!(
        SessionId, TurnId, ToolCallId, ApprovalId, SandboxId, ArtifactId, CommandId,
    );
}

#[test]
fn nil_id_deserialization_is_rejected() {
    let encoded = "\"00000000-0000-0000-0000-000000000000\"";
    let result = serde_json::from_str::<SessionId>(encoded);

    assert!(result.is_err());
}

#[test]
fn workspace_path_normalizes_and_is_idempotent() {
    let path = WorkspacePath::try_new("./src//./lib.rs").expect("valid path");

    assert_eq!(path.as_str(), "src/lib.rs");
    assert_eq!(WorkspacePath::try_new(path.as_str()), Ok(path));
}

#[test]
fn workspace_path_rejects_boundary_inputs() {
    let cases = [
        ("", WorkspacePathError::Empty),
        (".", WorkspacePathError::Empty),
        ("/etc/passwd", WorkspacePathError::Absolute),
        ("a/../b", WorkspacePathError::ParentTraversal),
        ("a\0b", WorkspacePathError::ContainsNul),
        ("a\\b", WorkspacePathError::BackslashNotAllowed),
        ("C:/workspace/file", WorkspacePathError::DrivePrefix),
    ];

    for (input, expected) in cases {
        assert_eq!(WorkspacePath::try_new(input), Err(expected));
    }
}

#[test]
fn workspace_path_deserialization_revalidates() {
    let encoded = "\"../secret\"";
    let result = serde_json::from_str::<WorkspacePath>(encoded);

    assert!(result.is_err());
    let valid = WorkspacePath::try_new("src/main.rs").expect("valid path");
    let round_trip = serde_json::from_str::<WorkspacePath>(
        &serde_json::to_string(&valid).expect("path serialization"),
    )
    .expect("path deserialization");
    assert_eq!(round_trip, valid);
}

#[test]
fn workspace_path_rejects_overlong_input() {
    let input = format!("a{}", "x".repeat(MAX_WORKSPACE_PATH_BYTES));

    assert_eq!(
        WorkspacePath::try_new(input),
        Err(WorkspacePathError::TooLong {
            max_bytes: MAX_WORKSPACE_PATH_BYTES,
            actual_bytes: MAX_WORKSPACE_PATH_BYTES + 1,
        }),
    );
}

#[test]
fn event_seq_overflow_is_typed() {
    assert_eq!(
        EventSeq::new(u64::MAX).checked_next(),
        Err(EventSeqError::Overflow),
    );
    assert_eq!(EventSeq::initial().value(), 0);
    assert_eq!(EventSeq::new(41).checked_next().unwrap().value(), 42);

    let _: DomainError = EventSeqError::Overflow.into();
}

#[test]
fn malformed_values_return_errors_without_panicking() {
    let values = ["\0", "..", "../", "/", "C:/", "a\\b", "////"];

    for value in values {
        let result = std::panic::catch_unwind(|| WorkspacePath::try_new(value));
        assert!(result.is_ok(), "validation panicked for {value:?}");
        assert!(result.expect("panic was checked").is_err());
    }

    let _: DomainError = IdError::Nil {
        id_type: "SessionId",
    }
    .into();
}
