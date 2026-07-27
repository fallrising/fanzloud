use serde::{Deserialize, Serialize};

use crate::EventSeqError;

/// A strongly typed, zero-based session event sequence number.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct EventSeq(u64);

impl EventSeq {
    /// The sequence before the first persisted event.
    ///
    /// Contract: `CU-FS-00`. This is an in-memory value with no persistence side effect.
    pub const fn initial() -> Self {
        Self(0)
    }

    /// Wraps an already-decoded sequence number.
    ///
    /// Contract: `CU-FS-00`. Every `u64` is a representable sequence value; no arithmetic is
    /// performed here.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric sequence value.
    ///
    /// Contract: `CU-FS-00`. This projection does not authorize or persist an event.
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Advances the sequence without allowing integer wraparound.
    ///
    /// Contract: `CU-FS-00`. Overflow is returned as a typed error and leaves this value
    /// unchanged.
    pub fn checked_next(self) -> Result<Self, EventSeqError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(EventSeqError::Overflow)
    }
}
