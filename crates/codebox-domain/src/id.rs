use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::IdError;

macro_rules! define_uuid_id {
    ($name:ident) => {
        /// A validated, non-nil identifier for one Codebox domain entity.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            const TYPE_NAME: &'static str = stringify!($name);

            /// Creates a fresh non-nil entity identifier.
            ///
            /// Contract: `CU-FS-00`. The constructor has no persistent or external side effect.
            pub fn new() -> Self {
                loop {
                    let value = Uuid::new_v4();
                    if !value.is_nil() {
                        return Self(value);
                    }
                }
            }

            /// Validates and wraps a UUID supplied across a trust boundary.
            ///
            /// Contract: `CU-FS-00`. Nil input returns `IdError::Nil` and is never wrapped.
            pub fn try_from_uuid(value: Uuid) -> Result<Self, IdError> {
                if value.is_nil() {
                    return Err(IdError::Nil {
                        id_type: Self::TYPE_NAME,
                    });
                }

                Ok(Self(value))
            }

            /// Returns the underlying UUID without changing it.
            ///
            /// Contract: `CU-FS-00`. This is a representation projection, not an authorization
            /// or ownership check.
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl TryFrom<Uuid> for $name {
            type Error = IdError;

            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                Self::try_from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                self.0.serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = Uuid::deserialize(deserializer)?;
                Self::try_from_uuid(value)
                    .map_err(|error| serde::de::Error::custom(error.to_string()))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

define_uuid_id!(SessionId);
define_uuid_id!(TurnId);
define_uuid_id!(ToolCallId);
define_uuid_id!(ApprovalId);
define_uuid_id!(SandboxId);
define_uuid_id!(ArtifactId);
define_uuid_id!(CommandId);
