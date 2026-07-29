use async_trait::async_trait;
use thiserror::Error;

use halo_crypto::TlsChannelBinding;
use halo_protocol::{MAX_FRAME_LEN, PairingMessage, ProtocolError};

/// One ordered, authenticated control stream. Implementations must disable
/// 0-RTT and return the exporter for the same QUIC/TLS connection.
#[async_trait]
pub trait ControlIo: Send {
    fn channel_binding(&self) -> TlsChannelBinding;
    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), FrameIoError>;
    async fn receive_frame(&mut self, max_len: usize) -> Result<Vec<u8>, FrameIoError>;
    async fn close(&mut self);
}

pub async fn send_message(
    io: &mut dyn ControlIo,
    message: &PairingMessage,
) -> Result<(), FrameIoError> {
    let frame = message.encode();
    if frame.len() > MAX_FRAME_LEN {
        return Err(FrameIoError::FrameTooLarge(frame.len()));
    }
    io.send_frame(&frame).await
}

pub async fn receive_message(io: &mut dyn ControlIo) -> Result<PairingMessage, FrameIoError> {
    let frame = io.receive_frame(MAX_FRAME_LEN).await?;
    if frame.len() > MAX_FRAME_LEN {
        return Err(FrameIoError::FrameTooLarge(frame.len()));
    }
    PairingMessage::decode(&frame).map_err(FrameIoError::Protocol)
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameIoError {
    #[error("control frame exceeds the configured limit: {0} bytes")]
    FrameTooLarge(usize),
    #[error("control stream ended before a complete frame")]
    Truncated,
    #[error("control stream read failed")]
    Read,
    #[error("control stream write failed")]
    Write,
    #[error("control protocol rejected a frame: {0}")]
    Protocol(ProtocolError),
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use halo_protocol::{
        Capabilities, ClientHello, IDENTITY_KEY_LEN, NONCE_LEN, ProtocolRange, SIGNATURE_LEN,
    };

    struct MemoryIo {
        frames: VecDeque<Vec<u8>>,
    }

    #[async_trait]
    impl ControlIo for MemoryIo {
        fn channel_binding(&self) -> TlsChannelBinding {
            TlsChannelBinding::new([1; 32])
        }

        async fn send_frame(&mut self, frame: &[u8]) -> Result<(), FrameIoError> {
            self.frames.push_back(frame.to_vec());
            Ok(())
        }

        async fn receive_frame(&mut self, _max_len: usize) -> Result<Vec<u8>, FrameIoError> {
            self.frames.pop_front().ok_or(FrameIoError::Truncated)
        }

        async fn close(&mut self) {}
    }

    #[tokio::test]
    async fn framed_messages_round_trip_through_bounded_io() {
        let message = PairingMessage::ClientHello(ClientHello {
            versions: ProtocolRange::new(1, 1)
                .unwrap_or_else(|error| panic!("test range: {error}")),
            capabilities: Capabilities::default(),
            nonce: [1; NONCE_LEN],
            identity_key: [2; IDENTITY_KEY_LEN],
            signature: [3; SIGNATURE_LEN],
        });
        let mut io = MemoryIo {
            frames: VecDeque::new(),
        };
        send_message(&mut io, &message)
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        let received = receive_message(&mut io)
            .await
            .unwrap_or_else(|error| panic!("receive: {error}"));
        assert_eq!(received, message);
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_protocol_decode() {
        let mut io = MemoryIo {
            frames: VecDeque::from([vec![0; MAX_FRAME_LEN + 1]]),
        };
        assert_eq!(
            receive_message(&mut io).await,
            Err(FrameIoError::FrameTooLarge(MAX_FRAME_LEN + 1))
        );
    }
}
