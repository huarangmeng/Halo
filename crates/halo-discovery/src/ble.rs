//! Platform-neutral BLE rendezvous constants and Presence descriptor codec.
//!
//! Scanning, advertising, and GATT lifecycle remain in platform-native code.
//! This module ensures every adapter exchanges the exact same bytes.

use std::time::Duration;

use thiserror::Error;

use crate::{
    LocalPresence, Observation, ProviderId,
    wire::{MessageKind, PACKET_LEN, PresenceMessage, WireError},
};

/// Fixed 128-bit GATT service UUID owned by the Halo protocol.
pub const HALO_SERVICE_UUID: &str = "b6882c7f-d426-4cb6-9012-d40bde5e2000";
/// Read/notify characteristic containing a fixed Presence v1 packet.
pub const PRESENCE_CHARACTERISTIC_UUID: &str = "8c2e5e61-4c6a-4c64-b804-1301a15251a0";
/// Write/notify characteristic used to trigger LAN Presence announcements.
pub const WAKE_LAN_CHARACTERISTIC_UUID: &str = "4fe6e851-dbc1-4a86-8e49-fcf1eabc1c82";
/// Optional read characteristic for bounded, untrusted endpoint hints.
pub const ENDPOINT_HINTS_CHARACTERISTIC_UUID: &str = "4672307b-caea-4e1a-8823-0bcea898ec83";

/// Encodes the value served by the BLE Presence characteristic.
#[must_use]
pub fn encode_presence(local: &LocalPresence, sequence: u64) -> [u8; PACKET_LEN] {
    PresenceMessage::from_local(local, MessageKind::Announce, sequence, 0).encode()
}

/// Validates a BLE Presence characteristic and maps it into the shared model.
pub fn decode_presence(
    bytes: &[u8],
    provider: ProviderId,
    ttl: Duration,
) -> Result<Observation, BleDescriptorError> {
    if ttl.is_zero() {
        return Err(BleDescriptorError::ZeroTtl);
    }
    let message = PresenceMessage::decode(bytes)?;
    if message.kind != MessageKind::Announce {
        return Err(BleDescriptorError::UnexpectedKind(message.kind));
    }
    Ok(Observation {
        provider,
        presence_id: message.presence_id,
        protocol: message.protocol,
        capabilities: message.capabilities,
        sequence: message.sequence,
        endpoints: Vec::new(),
        ttl,
        round_trip_time: None,
    })
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum BleDescriptorError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("BLE Presence characteristic must contain an announce packet, got {0:?}")]
    UnexpectedKind(MessageKind),
    #[error("BLE Presence observation TTL must be non-zero")]
    ZeroTtl,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capabilities, PresenceId, ProtocolRange, ProviderKind};

    #[test]
    fn ble_presence_uses_shared_strict_codec() {
        let local = LocalPresence::new(
            PresenceId::from_bytes([0x22; 16]),
            ProtocolRange::new(1, 2).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::from_bits(7),
            4433,
        )
        .unwrap_or_else(|error| panic!("local: {error}"));
        let provider = ProviderId::new(ProviderKind::Ble, "ble-ios")
            .unwrap_or_else(|error| panic!("provider: {error}"));
        let observation = decode_presence(
            &encode_presence(&local, 42),
            provider.clone(),
            Duration::from_secs(10),
        )
        .unwrap_or_else(|error| panic!("decode: {error}"));

        assert_eq!(observation.provider, provider);
        assert_eq!(observation.presence_id, local.presence_id);
        assert_eq!(observation.sequence, 42);
        assert!(observation.endpoints.is_empty());
    }
}
