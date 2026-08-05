use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use halo_crypto::TlsChannelBinding;
use halo_protocol::{HEADER_LEN, MAX_FRAME_LEN};

use crate::{ControlIo, FrameIoError};

const FRAME_QUEUE_CAPACITY: usize = 4;

/// Platform-owned half of a bounded control stream bridge. It carries complete
/// Halo frames only; native connection objects and addresses never enter Rust.
pub struct PlatformControlDriver {
    inbound: mpsc::Sender<Vec<u8>>,
    outbound: mpsc::Receiver<Vec<u8>>,
    cancellation: CancellationToken,
}

impl PlatformControlDriver {
    pub fn try_submit_frame(&self, frame: Vec<u8>) -> Result<(), PlatformControlError> {
        validate_frame_length(frame.len())?;
        self.inbound.try_send(frame).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => PlatformControlError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => PlatformControlError::Closed,
        })
    }

    pub fn drain_outbound(
        &mut self,
        maximum_frames: usize,
    ) -> Result<Vec<Vec<u8>>, PlatformControlError> {
        if !(1..=FRAME_QUEUE_CAPACITY).contains(&maximum_frames) {
            return Err(PlatformControlError::InvalidDrainLimit);
        }
        let mut frames = Vec::with_capacity(maximum_frames);
        while frames.len() < maximum_frames {
            match self.outbound.try_recv() {
                Ok(frame) => frames.push(frame),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) if frames.is_empty() => {
                    return Err(PlatformControlError::Closed);
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        Ok(frames)
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

pub struct PlatformControlIo {
    binding: TlsChannelBinding,
    inbound: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Vec<u8>>,
    cancellation: CancellationToken,
}

/// Creates the two process-local halves of a native QUIC control bridge.
#[must_use]
pub fn platform_control_channel(
    binding: TlsChannelBinding,
) -> (PlatformControlDriver, PlatformControlIo) {
    let (inbound_sender, inbound_receiver) = mpsc::channel(FRAME_QUEUE_CAPACITY);
    let (outbound_sender, outbound_receiver) = mpsc::channel(FRAME_QUEUE_CAPACITY);
    let cancellation = CancellationToken::new();
    (
        PlatformControlDriver {
            inbound: inbound_sender,
            outbound: outbound_receiver,
            cancellation: cancellation.clone(),
        },
        PlatformControlIo {
            binding,
            inbound: inbound_receiver,
            outbound: outbound_sender,
            cancellation,
        },
    )
}

#[async_trait]
impl ControlIo for PlatformControlIo {
    fn channel_binding(&self) -> TlsChannelBinding {
        self.binding
    }

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), FrameIoError> {
        validate_frame_length(frame.len()).map_err(|_| FrameIoError::FrameTooLarge(frame.len()))?;
        tokio::select! {
            () = self.cancellation.cancelled() => Err(FrameIoError::Write),
            result = self.outbound.send(frame.to_vec()) => {
                result.map_err(|_| FrameIoError::Write)
            }
        }
    }

    async fn receive_frame(&mut self, max_len: usize) -> Result<Vec<u8>, FrameIoError> {
        let frame = tokio::select! {
            () = self.cancellation.cancelled() => return Err(FrameIoError::Read),
            frame = self.inbound.recv() => frame.ok_or(FrameIoError::Truncated)?,
        };
        if frame.len() > max_len.min(MAX_FRAME_LEN) {
            return Err(FrameIoError::FrameTooLarge(frame.len()));
        }
        if frame.len() < HEADER_LEN {
            return Err(FrameIoError::Truncated);
        }
        Ok(frame)
    }

    async fn close(&mut self) {
        self.cancellation.cancel();
    }
}

fn validate_frame_length(length: usize) -> Result<(), PlatformControlError> {
    if !(HEADER_LEN..=MAX_FRAME_LEN).contains(&length) {
        return Err(PlatformControlError::InvalidFrameLength(length));
    }
    Ok(())
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PlatformControlError {
    #[error("native control frame length is invalid: {0}")]
    InvalidFrameLength(usize),
    #[error("native control frame queue is full")]
    QueueFull,
    #[error("native control stream is closed")]
    Closed,
    #[error("native control drain limit is invalid")]
    InvalidDrainLimit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(byte: u8) -> Vec<u8> {
        vec![byte; HEADER_LEN]
    }

    #[tokio::test]
    async fn platform_bridge_moves_bounded_frames_in_both_directions() {
        let (mut driver, mut io) = platform_control_channel(TlsChannelBinding::new([0x42; 32]));
        driver
            .try_submit_frame(frame(1))
            .unwrap_or_else(|error| panic!("submit: {error}"));
        assert_eq!(
            io.receive_frame(MAX_FRAME_LEN)
                .await
                .unwrap_or_else(|error| panic!("receive: {error}")),
            frame(1)
        );

        io.send_frame(&frame(2))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        assert_eq!(
            driver
                .drain_outbound(1)
                .unwrap_or_else(|error| panic!("drain: {error}")),
            vec![frame(2)]
        );
        assert_eq!(io.channel_binding(), TlsChannelBinding::new([0x42; 32]));
    }

    #[test]
    fn platform_bridge_rejects_unbounded_input_and_queue_growth() {
        let (mut driver, _io) = platform_control_channel(TlsChannelBinding::new([0; 32]));
        assert_eq!(
            driver.try_submit_frame(vec![0; HEADER_LEN - 1]),
            Err(PlatformControlError::InvalidFrameLength(HEADER_LEN - 1))
        );
        assert_eq!(
            driver.try_submit_frame(vec![0; MAX_FRAME_LEN + 1]),
            Err(PlatformControlError::InvalidFrameLength(MAX_FRAME_LEN + 1))
        );
        for index in 0..FRAME_QUEUE_CAPACITY {
            driver
                .try_submit_frame(frame(index as u8))
                .unwrap_or_else(|error| panic!("fill queue: {error}"));
        }
        assert_eq!(
            driver.try_submit_frame(frame(9)),
            Err(PlatformControlError::QueueFull)
        );
        assert_eq!(
            driver.drain_outbound(0),
            Err(PlatformControlError::InvalidDrainLimit)
        );
    }

    #[tokio::test]
    async fn cancelling_driver_releases_waiting_rust_io() {
        let (driver, mut io) = platform_control_channel(TlsChannelBinding::new([0; 32]));
        driver.cancel();
        assert_eq!(
            io.receive_frame(MAX_FRAME_LEN).await,
            Err(FrameIoError::Read)
        );
    }
}
