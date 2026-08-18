use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use halo_crypto::TlsChannelBinding;
use halo_protocol::{BatchChunkRef, DEFAULT_CHUNK_SIZE, ResumePosition};
use halo_transfer::{
    BatchResumeStore, BatchSource, prepare_batch_with_id, receive_batch_data_with_progress,
    send_batch_data_with_progress,
};
use halo_transport::{DataIo, DataIoError};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

const DEFAULT_MIB: u64 = 256;
const MAX_MIB: u64 = 4096;

struct CountingDataIo {
    binding: TlsChannelBinding,
    payload_bytes: u64,
    records: u64,
}

struct PipeDataIo {
    binding: TlsChannelBinding,
    sender: Option<mpsc::Sender<Vec<u8>>>,
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
}

#[async_trait]
impl DataIo for CountingDataIo {
    fn channel_binding(&self) -> TlsChannelBinding {
        self.binding
    }

    async fn send_record(&mut self, record: &[u8]) -> Result<(), DataIoError> {
        let chunk = BatchChunkRef::decode(record).map_err(|_| DataIoError::InvalidMagic)?;
        self.payload_bytes = self
            .payload_bytes
            .checked_add(chunk.payload.len() as u64)
            .ok_or(DataIoError::RecordTooLarge(usize::MAX))?;
        self.records = self
            .records
            .checked_add(1)
            .ok_or(DataIoError::RecordTooLarge(usize::MAX))?;
        Ok(())
    }

    async fn receive_record(&mut self) -> Result<Vec<u8>, DataIoError> {
        Err(DataIoError::Read)
    }

    async fn finish_send(&mut self) -> Result<(), DataIoError> {
        Ok(())
    }

    async fn expect_end(&mut self) -> Result<(), DataIoError> {
        Ok(())
    }

    async fn close(&mut self) {}
}

#[async_trait]
impl DataIo for PipeDataIo {
    fn channel_binding(&self) -> TlsChannelBinding {
        self.binding
    }

    async fn send_record(&mut self, record: &[u8]) -> Result<(), DataIoError> {
        self.sender
            .as_ref()
            .ok_or(DataIoError::Write)?
            .send(record.to_vec())
            .await
            .map_err(|_| DataIoError::Write)
    }

    async fn receive_record(&mut self) -> Result<Vec<u8>, DataIoError> {
        self.receiver
            .as_mut()
            .ok_or(DataIoError::Read)?
            .recv()
            .await
            .ok_or(DataIoError::Truncated)
    }

    async fn finish_send(&mut self) -> Result<(), DataIoError> {
        self.sender.take();
        Ok(())
    }

    async fn expect_end(&mut self) -> Result<(), DataIoError> {
        match self
            .receiver
            .as_mut()
            .ok_or(DataIoError::Read)?
            .recv()
            .await
        {
            None => Ok(()),
            Some(_) => Err(DataIoError::TrailingData),
        }
    }

    async fn close(&mut self) {
        self.sender.take();
        self.receiver.take();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mib = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(DEFAULT_MIB);
    if mib == 0 || mib > MAX_MIB {
        return Err(format!("size must be between 1 and {MAX_MIB} MiB").into());
    }

    let byte_count = mib * 1024 * 1024;
    let root = benchmark_directory();
    fs::create_dir_all(&root).await?;
    let source = root.join("payload.bin");
    create_source(&source, byte_count).await?;

    let cancellation = CancellationToken::new();
    let prepare_started = Instant::now();
    let prepared = prepare_batch_with_id(
        [0x41; 16],
        vec![BatchSource::new(&source, None)],
        &cancellation,
    )
    .await?;
    let prepare_elapsed = prepare_started.elapsed();

    let binding = TlsChannelBinding::new([0x42; 32]);
    let mut data = CountingDataIo {
        binding,
        payload_bytes: 0,
        records: 0,
    };
    let positions = [ResumePosition {
        file_index: 0,
        next_chunk_index: 0,
    }];
    let send_started = Instant::now();
    send_batch_data_with_progress(
        &mut data,
        binding,
        &prepared,
        &positions,
        &cancellation,
        |_, _, _| {},
    )
    .await?;
    let send_elapsed = send_started.elapsed();

    let resume = root.join("resume");
    let destination = root.join("destination");
    fs::create_dir_all(&resume).await?;
    fs::create_dir_all(&destination).await?;
    let resume_store = BatchResumeStore::new(&resume, [0x43; 32]);
    let (pipe_sender, pipe_receiver) = data_pipe(binding);
    let mut pipe_sender = pipe_sender;
    let mut pipe_receiver = pipe_receiver;
    let pipeline_started = Instant::now();
    let (sent, received) = tokio::join!(
        send_batch_data_with_progress(
            &mut pipe_sender,
            binding,
            &prepared,
            &positions,
            &cancellation,
            |_, _, _| {},
        ),
        receive_batch_data_with_progress(
            &mut pipe_receiver,
            binding,
            prepared.manifest(),
            &resume_store,
            &destination,
            &cancellation,
            |_, _, _| {},
        ),
    );
    sent?;
    let received = received?;
    let pipeline_elapsed = pipeline_started.elapsed();

    println!(
        concat!(
            "payload={} MiB chunk={} KiB records={} prepare={:.1} MiB/s ",
            "send={:.1} MiB/s pipeline={:.1} MiB/s"
        ),
        mib,
        DEFAULT_CHUNK_SIZE / 1024,
        data.records,
        throughput_mib(data.payload_bytes, prepare_elapsed),
        throughput_mib(data.payload_bytes, send_elapsed),
        throughput_mib(received.aggregate_size, pipeline_elapsed),
    );
    fs::remove_dir_all(root).await?;
    Ok(())
}

fn data_pipe(binding: TlsChannelBinding) -> (PipeDataIo, PipeDataIo) {
    let (sender, receiver) = mpsc::channel(4);
    (
        PipeDataIo {
            binding,
            sender: Some(sender),
            receiver: None,
        },
        PipeDataIo {
            binding,
            sender: None,
            receiver: Some(receiver),
        },
    )
}

async fn create_source(path: &PathBuf, byte_count: u64) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(path).await?;
    let buffer = vec![0x5a; DEFAULT_CHUNK_SIZE as usize];
    let mut remaining = byte_count;
    while remaining > 0 {
        let length = usize::try_from(remaining.min(buffer.len() as u64))?;
        file.write_all(&buffer[..length]).await?;
        remaining -= length as u64;
    }
    file.flush().await?;
    file.sync_all().await?;
    Ok(())
}

fn benchmark_directory() -> PathBuf {
    std::env::temp_dir().join(format!("halo-transfer-benchmark-{}", std::process::id()))
}

fn throughput_mib(bytes: u64, elapsed: Duration) -> f64 {
    bytes as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
}
