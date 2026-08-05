# Halo single-file transfer protocol v1

- Status: Experimental
- Wire version: 1

This protocol runs only after pairing has authenticated both device identities
on the same QUIC connection. QUIC 0-RTT is disabled. One control stream carries
the offer and decision; a separate data stream carries bounded file chunks.

## Limits

| Item | v1 limit |
| --- | ---: |
| UTF-8 filename | 255 bytes |
| File size | 10 TiB |
| Chunk payload | 256 KiB maximum; 64 KiB default |
| Active transfers | one per authenticated session |
| Control frame | 4096 bytes including the common header |
| Data-record header | 60 bytes |

## Common control header

Transfer control messages reuse the deterministic Halo header:

```text
magic[4] = "HALO"
wire_version: u16 big-endian = 1
kind: u8
flags: u8 = 0
payload_length: u32 big-endian
payload[payload_length]
```

Unknown kinds, non-zero flags, unsupported versions, length mismatches, and
over-limit frames are fatal for the transfer stream.

## Control messages

All integers are big-endian. `transfer_id` is 16 unpredictable bytes generated
by the sender. Digests are 32-byte SHA-256 values.

### Offer (`kind = 16`)

```text
transfer_id[16]
file_size: u64
chunk_size: u32
file_digest[32]
filename_length: u16
filename[filename_length]  // strict UTF-8 leaf name
```

### Decision (`kind = 17`)

```text
transfer_id[16]
accepted: u8               // exactly 0 or 1
```

### Complete (`kind = 18`)

```text
transfer_id[16]
file_digest[32]
```

`Complete` is sent by the receiver only after durable chunk writes, whole-file
verification, and successful no-overwrite finalization.

### Cancel (`kind = 19`)

```text
transfer_id[16]
reason: u8
```

Reason values are stable categories: `1` user cancellation, `2` policy, `3`
integrity, `4` storage, and `5` protocol. Unknown reason values are rejected.

## Data stream

The accepted sender opens one data stream. It contains consecutive chunk
records and no padding:

```text
magic[4] = "HDF1"
transfer_id[16]
chunk_index: u32
payload_length: u32
chunk_digest[32]
payload[payload_length]
```

All integer fields are big-endian. The receiver reads and validates the fixed
60-byte header before allocating the payload. `payload_length` must be in
`1..=262144`, and the complete record must be exactly
`60 + payload_length` bytes; trailing bytes are not part of that record.

The first index is zero and indices are contiguous. Every non-final chunk must
match the offered chunk size; the final chunk is exactly the remaining byte
count. Zero-length files contain no chunk records. Extra, missing, reordered,
oversized, or digest-mismatched records fail the transfer.

## State machine

```text
sender:   Offer -> Decision(accept) -> chunks -> wait Complete
receiver: Offer -> user/policy decision -> Decision(accept) -> chunks
          -> verify -> finalize -> Complete
```

A rejection ends the control stream without a data stream. Cancellation or any
error closes the affected streams and removes partial staging state. It never
creates or replaces a final destination file.
