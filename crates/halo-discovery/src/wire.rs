//! Fixed-width Halo Presence Protocol v1 codec.

use thiserror::Error;

use crate::{Capabilities, LocalPresence, PresenceId, ProtocolRange};

pub const PACKET_LEN: usize = 58;
pub const WIRE_VERSION: u8 = 1;
const MAGIC: &[u8; 8] = b"HALODSC1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MessageKind {
    Query = 1,
    Response = 2,
    Announce = 3,
    Goodbye = 4,
}

impl TryFrom<u8> for MessageKind {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Query),
            2 => Ok(Self::Response),
            3 => Ok(Self::Announce),
            4 => Ok(Self::Goodbye),
            _ => Err(WireError::UnknownKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresenceMessage {
    pub kind: MessageKind,
    /// UDP port where a query expects its unicast response. Zero for every
    /// non-query message.
    pub reply_port: u16,
    pub presence_id: PresenceId,
    pub quic_port: u16,
    pub protocol: ProtocolRange,
    pub capabilities: Capabilities,
    pub sequence: u64,
    pub nonce: u64,
}

impl PresenceMessage {
    #[must_use]
    pub fn from_local(local: &LocalPresence, kind: MessageKind, sequence: u64, nonce: u64) -> Self {
        Self {
            kind,
            reply_port: 0,
            presence_id: local.presence_id,
            quic_port: local.quic_port,
            protocol: local.protocol,
            capabilities: local.capabilities,
            sequence,
            nonce,
        }
    }

    /// Sets the response port carried by a query.
    #[must_use]
    pub const fn with_reply_port(mut self, reply_port: u16) -> Self {
        self.reply_port = reply_port;
        self
    }

    #[must_use]
    pub fn encode(self) -> [u8; PACKET_LEN] {
        let mut bytes = [0_u8; PACKET_LEN];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8] = WIRE_VERSION;
        bytes[9] = self.kind as u8;
        bytes[10..12].copy_from_slice(&self.reply_port.to_be_bytes());
        bytes[12..28].copy_from_slice(self.presence_id.as_bytes());
        bytes[28..30].copy_from_slice(&self.quic_port.to_be_bytes());
        bytes[30..32].copy_from_slice(&self.protocol.min().to_be_bytes());
        bytes[32..34].copy_from_slice(&self.protocol.max().to_be_bytes());
        bytes[34..42].copy_from_slice(&self.capabilities.bits().to_be_bytes());
        bytes[42..50].copy_from_slice(&self.sequence.to_be_bytes());
        bytes[50..58].copy_from_slice(&self.nonce.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != PACKET_LEN {
            return Err(WireError::InvalidLength(bytes.len()));
        }
        if &bytes[..8] != MAGIC {
            return Err(WireError::InvalidMagic);
        }
        if bytes[8] != WIRE_VERSION {
            return Err(WireError::UnsupportedVersion(bytes[8]));
        }
        let kind = MessageKind::try_from(bytes[9])?;
        let reply_port = u16::from_be_bytes(copy_array::<2>(&bytes[10..12]));
        if (kind == MessageKind::Query) != (reply_port != 0) {
            return Err(WireError::InvalidReplyPort);
        }
        let presence_id = PresenceId::from_bytes(copy_array::<16>(&bytes[12..28]));
        let quic_port = u16::from_be_bytes(copy_array::<2>(&bytes[28..30]));
        if quic_port == 0 {
            return Err(WireError::InvalidPort);
        }
        let min = u16::from_be_bytes(copy_array::<2>(&bytes[30..32]));
        let max = u16::from_be_bytes(copy_array::<2>(&bytes[32..34]));
        let protocol = ProtocolRange::new(min, max).map_err(|_| WireError::InvalidProtocolRange)?;
        let capabilities =
            Capabilities::from_bits(u64::from_be_bytes(copy_array::<8>(&bytes[34..42])));
        let sequence = u64::from_be_bytes(copy_array::<8>(&bytes[42..50]));
        let nonce = u64::from_be_bytes(copy_array::<8>(&bytes[50..58]));

        Ok(Self {
            kind,
            reply_port,
            presence_id,
            quic_port,
            protocol,
            capabilities,
            sequence,
            nonce,
        })
    }
}

fn copy_array<const N: usize>(slice: &[u8]) -> [u8; N] {
    let mut output = [0_u8; N];
    output.copy_from_slice(slice);
    output
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WireError {
    #[error("presence packet has invalid length {0}")]
    InvalidLength(usize),
    #[error("presence packet magic does not match")]
    InvalidMagic,
    #[error("unsupported presence wire version {0}")]
    UnsupportedVersion(u8),
    #[error("unknown presence message kind {0}")]
    UnknownKind(u8),
    #[error("presence query must carry a reply port and other messages must not")]
    InvalidReplyPort,
    #[error("presence packet advertises port zero")]
    InvalidPort,
    #[error("presence packet has an invalid protocol range")]
    InvalidProtocolRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> LocalPresence {
        LocalPresence::new(
            PresenceId::from_bytes([0x11; 16]),
            ProtocolRange::new(1, 3).unwrap_or_else(|error| panic!("test range: {error}")),
            Capabilities::from_bits(0x0102_0304_0506_0708),
            4433,
        )
        .unwrap_or_else(|error| panic!("test presence: {error}"))
    }

    #[test]
    fn golden_query_vector_is_stable() {
        let encoded = PresenceMessage::from_local(
            &local(),
            MessageKind::Query,
            0x1112_1314_1516_1718,
            0x2122_2324_2526_2728,
        )
        .with_reply_port(44721)
        .encode();

        assert_eq!(encoded.len(), PACKET_LEN);
        assert_eq!(&encoded[..12], b"HALODSC1\x01\x01\xae\xb1");
        assert_eq!(&encoded[12..28], &[0x11; 16]);
        assert_eq!(&encoded[28..34], &[0x11, 0x51, 0, 1, 0, 3]);
        assert_eq!(&encoded[34..42], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            PresenceMessage::decode(&encoded),
            Ok(PresenceMessage::from_local(
                &local(),
                MessageKind::Query,
                0x1112_1314_1516_1718,
                0x2122_2324_2526_2728,
            )
            .with_reply_port(44721))
        );
    }

    #[test]
    fn rejects_every_truncated_length() {
        let encoded = PresenceMessage::from_local(&local(), MessageKind::Announce, 1, 0).encode();
        for length in 0..PACKET_LEN {
            assert_eq!(
                PresenceMessage::decode(&encoded[..length]),
                Err(WireError::InvalidLength(length))
            );
        }
    }

    #[test]
    fn rejects_reserved_bytes_and_invalid_fields() {
        let base = PresenceMessage::from_local(&local(), MessageKind::Announce, 1, 0).encode();

        let mut invalid = base;
        invalid[10..12].copy_from_slice(&1_u16.to_be_bytes());
        assert_eq!(
            PresenceMessage::decode(&invalid),
            Err(WireError::InvalidReplyPort)
        );

        let invalid = PresenceMessage::from_local(&local(), MessageKind::Query, 1, 2).encode();
        assert_eq!(
            PresenceMessage::decode(&invalid),
            Err(WireError::InvalidReplyPort)
        );

        let mut invalid = base;
        invalid[28..30].copy_from_slice(&0_u16.to_be_bytes());
        assert_eq!(
            PresenceMessage::decode(&invalid),
            Err(WireError::InvalidPort)
        );

        let mut invalid = base;
        invalid[30..32].copy_from_slice(&4_u16.to_be_bytes());
        invalid[32..34].copy_from_slice(&3_u16.to_be_bytes());
        assert_eq!(
            PresenceMessage::decode(&invalid),
            Err(WireError::InvalidProtocolRange)
        );
    }
}
