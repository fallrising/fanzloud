use chrono::{DateTime, SecondsFormat, Utc};
use codebox_domain::{
    DomainEvent, DomainEventEnvelope, EventSeq, IdError, NewDomainEvent, SessionId,
};
use rusqlite::Row;
use uuid::Uuid;

use crate::{CorruptStoreStage, EventStoreError, MAX_EVENT_PAYLOAD_BYTES};

pub(crate) struct EncodedNewEvent {
    pub(crate) schema_version: u16,
    pub(crate) occurred_at: String,
    pub(crate) causation_id: Option<[u8; 16]>,
    pub(crate) correlation_id: [u8; 16],
    pub(crate) payload: Vec<u8>,
}

pub(crate) struct RawStoredEvent {
    event_id: Vec<u8>,
    stream_id: Vec<u8>,
    seq: Vec<u8>,
    schema_version: i64,
    occurred_at: String,
    causation_id: Option<Vec<u8>>,
    correlation_id: Vec<u8>,
    payload: Vec<u8>,
}

pub(crate) fn encode_new(
    event: &NewDomainEvent,
    index: usize,
) -> Result<EncodedNewEvent, EventStoreError> {
    let payload =
        serde_json::to_vec(&event.payload).map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Payload,
        })?;
    validate_payload_size(index, payload.len())?;

    Ok(EncodedNewEvent {
        schema_version: event.schema_version,
        occurred_at: event
            .occurred_at
            .to_rfc3339_opts(SecondsFormat::Nanos, true),
        causation_id: event.causation_id.map(|value| *value.as_bytes()),
        correlation_id: *event.correlation_id.as_bytes(),
        payload,
    })
}

fn validate_payload_size(index: usize, actual: usize) -> Result<(), EventStoreError> {
    if actual > MAX_EVENT_PAYLOAD_BYTES {
        return Err(EventStoreError::EventPayloadTooLarge {
            index,
            max: MAX_EVENT_PAYLOAD_BYTES,
            actual,
        });
    }
    Ok(())
}

pub(crate) fn sequence_bytes(sequence: EventSeq) -> [u8; 8] {
    sequence.value().to_be_bytes()
}

pub(crate) fn decode_sequence(bytes: &[u8]) -> Result<EventSeq, EventStoreError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Sequence,
        })?;
    Ok(EventSeq::new(u64::from_be_bytes(encoded)))
}

pub(crate) fn raw_event_from_row(row: &Row<'_>) -> rusqlite::Result<RawStoredEvent> {
    Ok(RawStoredEvent {
        event_id: row.get(0)?,
        stream_id: row.get(1)?,
        seq: row.get(2)?,
        schema_version: row.get(3)?,
        occurred_at: row.get(4)?,
        causation_id: row.get(5)?,
        correlation_id: row.get(6)?,
        payload: row.get(7)?,
    })
}

pub(crate) fn decode_event(raw: RawStoredEvent) -> Result<DomainEventEnvelope, EventStoreError> {
    let event_id = decode_uuid(&raw.event_id, CorruptStoreStage::EventId)?;
    if event_id.is_nil() {
        return Err(EventStoreError::CorruptStore {
            stage: CorruptStoreStage::EventId,
        });
    }
    let stream_uuid = decode_uuid(&raw.stream_id, CorruptStoreStage::StreamId)?;
    let stream_id = SessionId::try_from_uuid(stream_uuid).map_err(|_: IdError| {
        EventStoreError::CorruptStore {
            stage: CorruptStoreStage::StreamId,
        }
    })?;
    let schema_version =
        u16::try_from(raw.schema_version).map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::SchemaVersion,
        })?;
    let occurred_at = DateTime::parse_from_rfc3339(&raw.occurred_at)
        .map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Timestamp,
        })?
        .with_timezone(&Utc);
    let causation_id = raw
        .causation_id
        .as_deref()
        .map(|bytes| decode_uuid(bytes, CorruptStoreStage::CausationId))
        .transpose()?;
    let correlation_id = decode_uuid(&raw.correlation_id, CorruptStoreStage::CorrelationId)?;
    let payload: DomainEvent =
        serde_json::from_slice(&raw.payload).map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Payload,
        })?;

    Ok(DomainEventEnvelope {
        event_id,
        stream_id,
        seq: decode_sequence(&raw.seq)?,
        schema_version,
        occurred_at,
        causation_id,
        correlation_id,
        payload,
    })
}

fn decode_uuid(bytes: &[u8], stage: CorruptStoreStage) -> Result<Uuid, EventStoreError> {
    Uuid::from_slice(bytes).map_err(|_| EventStoreError::CorruptStore { stage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_payload_limit_rejects_oversized_bytes() {
        assert_eq!(
            validate_payload_size(3, MAX_EVENT_PAYLOAD_BYTES + 1),
            Err(EventStoreError::EventPayloadTooLarge {
                index: 3,
                max: MAX_EVENT_PAYLOAD_BYTES,
                actual: MAX_EVENT_PAYLOAD_BYTES + 1,
            })
        );
        assert_eq!(validate_payload_size(0, MAX_EVENT_PAYLOAD_BYTES), Ok(()));
    }
}
