# Halo resumable multi-file transfer protocol v1

- Status: Experimental
- Application protocol version: 1
- Control frame limit: 4096 bytes

Transfer v1 runs only on a QUIC connection authenticated by Halo pairing that
negotiated application protocol version 1.

## Limits

| Item | v1 limit |
| --- | ---: |
| Files per manifest | 8 |
| UTF-8 filename | 255 bytes |
| Per-file size | 10 TiB |
| Aggregate size | 10 TiB |
| Chunk payload | 256 KiB maximum and current sender default |
| Active batches | one per authenticated session |

All integers are unsigned and big-endian. Control frames use the common
12-byte `HALO` header with wire version `1` and zero flags.

## Canonical manifest digest

The SHA-256 input is:

```text
"Halo Transfer Manifest v1"
transfer_id[16]
chunk_size: u32
file_count: u8
for each file in order:
  file_size: u64
  file_digest[32]
  filename_length: u16
  filename[filename_length]
```

The ordered digest binds every resume request and terminal message.

Golden manifest digest: transfer ID `11` repeated 16 times, chunk size 65536,
and files `(7, 21*32, "first.txt")`, `(9, 22*32, "second.bin")` produce:

```text
f7a79061ca4c5852e1d1a963286bec8355bc686b986522a8f5ef9f69a6ca3407
```

## Control messages

### Offer (`kind = 32`)

```text
transfer_id[16]
chunk_size: u32
file_count: u8
reserved[3] = 0
aggregate_size: u64
manifest_digest[32]
for each file:
  file_size: u64
  file_digest[32]
  filename_length: u16
  filename[filename_length]
```

The decoder recomputes aggregate size and manifest digest. Duplicate or unsafe
leaf names are rejected by the transfer engine before consent.

### Decision (`kind = 33`)

```text
transfer_id[16]
manifest_digest[32]
accepted: u8
position_count: u8
reserved: u16 = 0
for each position:
  file_index: u16
  next_chunk_index: u32
```

An accepted decision contains one ordered position per manifest file. A new
transfer uses zero for every position. A rejected decision has no positions.
Positions beyond a file's exact chunk count are fatal.

### Complete (`kind = 34`)

```text
transfer_id[16]
manifest_digest[32]
```

The receiver sends Complete only after all files pass chunk and whole-file
verification and no-overwrite finalization.

### Cancel (`kind = 35`)

```text
transfer_id[16]
reason: u8
```

Reasons are `1=user`, `2=policy`, `3=integrity`, `4=storage`, and `5=protocol`.
Cancel removes receiver partial state owned by this transfer.

### Pause (`kind = 36`)

```text
transfer_id[16]
manifest_digest[32]
reason: u8
```

Reasons are `1=user`, `2=route lost`, and `3=application lifecycle`. Pause
retains only already synchronized and verified partial state. Resuming requires
a new authenticated session when the old route or connection was lost.

## Data records

Accepted senders write ordered records with a 64-byte header:

```text
magic[4] = "HDF1"
transfer_id[16]
file_index: u16
reserved: u16 = 0
chunk_index: u32
payload_length: u32
chunk_digest[32]
payload[payload_length]
```

Records start at the receiver-provided position for each file. File indices are
in manifest order. Chunk indices are contiguous. Every non-final payload equals
the manifest chunk size; the final payload is the exact remainder. Files whose
position equals their chunk count send no records.

The receiver validates every chunk before writing it. It synchronizes private
partial data before atomically advancing durable resume state at 16 MiB
checkpoints and at every file boundary. Missing, duplicate, reordered,
appended, mismatched, or over-limit data fails the batch. An abrupt process or
power loss may require retransmitting at most the uncommitted 16 MiB suffix;
resume loading truncates that suffix before advertising a position. Every
completed whole-file digest is verified before finalization.

## State machine

```text
sender:   Offer -> Decision(accept + positions) -> records
          -> Complete
receiver: Offer -> consent/policy -> Decision -> records
          -> verify all -> finalize all -> Complete

either side after acceptance:
          -> Pause (retain verified private state)
          -> Cancel (remove private state)
```

Rejection, cancellation, timeout, malformed state, or failed finalization never
overwrites an existing destination. Resume state is scoped to the complete
authenticated peer key, transfer ID, and manifest digest.
