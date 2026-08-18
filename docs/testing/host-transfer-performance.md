# Host transfer performance check

This check measures the Rust sender hot path and a bounded host pipeline without
a physical device or network. It includes source reads, whole-file and
per-chunk SHA-256, record encoding, borrowed record decoding, async backpressure,
receiver writes, durability checkpoints, final verification, and no-overwrite
finalization. It does not claim Android/macOS network, thermal, or physical
end-to-end results.

Run an optimized 256 MiB sample:

```bash
cargo run --release -p halo-transfer --example host_transfer_benchmark -- 256
```

Record the hardware, OS, Rust version, payload size, chunk size, record count,
preparation throughput, sending throughput, and full host-pipeline throughput.
Compare results only on the same host with the same power, filesystem, and
thermal state.

The current protocol uses 256 KiB chunks and a 16 MiB receiver durability
checkpoint interval. A 10 GiB file therefore emits 40,960 data records and at
most 640 periodic durability checkpoints, plus final per-file synchronization.
An abrupt process or power loss may require retransmitting at most the
uncommitted 16 MiB suffix. Resume loading verifies the committed prefix and
truncates any uncommitted private tail before advertising a resume position.
