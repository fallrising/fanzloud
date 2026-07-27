use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::WorkspacePathError;

/// Maximum normalized UTF-8 length of a portable workspace path.
///
/// Contract: `CU-FS-00`. This is a value-layer bound, not a host filesystem limit.
pub const MAX_WORKSPACE_PATH_BYTES: usize = 4096;

/// A normalized, portable path relative to a Codebox workspace.
///
/// Contract: `CU-FS-00`. The value does not inspect or resolve a filesystem.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    /// Validates and normalizes a caller-supplied workspace-relative path.
    ///
    /// Contract: `CU-FS-00`. Absolute paths, parent traversal, NUL, backslash separators, drive
    /// prefixes, empty values, and overlong values return typed errors without panicking.
    pub fn try_new(input: impl AsRef<str>) -> Result<Self, WorkspacePathError> {
        let raw = input.as_ref();

        if raw.is_empty() {
            return Err(WorkspacePathError::Empty);
        }
        if raw.contains('\0') {
            return Err(WorkspacePathError::ContainsNul);
        }
        if raw.contains('\\') {
            return Err(WorkspacePathError::BackslashNotAllowed);
        }
        if raw.starts_with('/') {
            return Err(WorkspacePathError::Absolute);
        }
        if is_drive_prefixed(raw) {
            return Err(WorkspacePathError::DrivePrefix);
        }
        if raw.len() > MAX_WORKSPACE_PATH_BYTES {
            return Err(WorkspacePathError::TooLong {
                max_bytes: MAX_WORKSPACE_PATH_BYTES,
                actual_bytes: raw.len(),
            });
        }

        let mut normalized = String::with_capacity(raw.len());
        for component in raw.split('/') {
            match component {
                "" | "." => {}
                ".." => return Err(WorkspacePathError::ParentTraversal),
                component => {
                    if !normalized.is_empty() {
                        normalized.push('/');
                    }
                    normalized.push_str(component);
                }
            }
        }

        if normalized.is_empty() {
            return Err(WorkspacePathError::Empty);
        }
        if normalized.len() > MAX_WORKSPACE_PATH_BYTES {
            return Err(WorkspacePathError::TooLong {
                max_bytes: MAX_WORKSPACE_PATH_BYTES,
                actual_bytes: normalized.len(),
            });
        }

        Ok(Self(normalized))
    }

    /// Returns the normalized portable path.
    ///
    /// Contract: `CU-FS-00`. The returned string is not a filesystem resolution or authorization
    /// result.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for WorkspacePath {
    type Error = WorkspacePathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl TryFrom<String> for WorkspacePath {
    type Error = WorkspacePathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl AsRef<str> for WorkspacePath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Serialize for WorkspacePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorkspacePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(|error| serde::de::Error::custom(error.to_string()))
    }
}

impl std::fmt::Display for WorkspacePath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn is_drive_prefixed(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
