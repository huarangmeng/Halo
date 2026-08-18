# ADR 0011: Resumable multi-file transfer protocol

- Status: Accepted
- Date: 2026-08-17
- Owners: Halo maintainers

## Context

An earlier, unpublished implementation used a single-file transfer draft. The
active Android/macOS milestone requires one-consent multi-file offers, pause,
retry, and restart-safe resume. Halo has not shipped a product or protocol
release, so maintaining two transfer formats would add branching and test cost
without preserving compatibility for any released peer.

Resume data is untrusted after process or route loss. A stale or substituted
partial file must not be combined with a new manifest, peer, or transfer.

## Decision

Halo has one application protocol version:

- Pairing v1 authenticates identities and negotiates application protocol
  version 1.
- Every authenticated session uses `protocol/transfer-v1.md`.
- Discovery advertises application versions `1..=1`; this is not a trust claim.
- The unpublished single-file wire draft is removed rather than retained as a
  compatibility mode.

One manifest contains one to eight files. It binds a random transfer ID,
fixed chunk size, ordered file names, sizes, and SHA-256 digests. The aggregate
size cannot exceed the protocol maximum. Filenames remain conservative leaf
names and duplicate names are rejected.

The receiver persists only app-private resume state:

- authenticated peer ID
- transfer ID and canonical manifest digest
- ordered per-file next chunk indices
- rolling digests for the durable chunk prefixes
- finalization phase

The receiver groups verified chunks into durability checkpoints of at most
16 MiB. It synchronizes the partial file before atomically replacing the
progress record, and always checkpoints a completed file. On load, Halo
verifies the peer, manifest, committed partial length, and rolling digest before
advertising a resume position. Extra uncommitted bytes are truncated only when
the authenticated state proves ownership of that partial file. An abrupt
process or power loss can therefore require retransmitting at most 16 MiB.
Malformed, mismatched, or over-limit state fails closed.

Pause preserves verified partials. Cancel removes them. Route loss pauses the
job and requires a new eligible local path, a new QUIC connection, and fresh
Halo authentication before the same manifest can resume. QUIC migration does
not resume a transfer.

The initial scheduler runs one batch per authenticated session and sends
files and chunks in manifest order. Resume positions are therefore contiguous
prefixes, not arbitrary sparse bitmaps. The durable model records one prefix
position per file; a future sparse scheduler requires another negotiated
capability or protocol version.

Final names are created with no-overwrite hard links only after every file is
verified. If in-process multi-file finalization fails, Halo removes only links
created by that attempt and retains verified private staging for retry.

## Consequences

- There is no runtime protocol-version branch in transfer, Flutter, FFI, or
  platform framing.
- The protocol requires bounded manifest parsing, resume-state parsing, and
  restart/failure-injection tests.
- Source files are copied into platform-private outgoing storage before Rust
  accepts a job, so retry never depends on a transient picker grant.
- A sender restart can resume only while its private source job still exists.
- A receiver may discard stale partial state under a bounded retention policy;
  resume is recoverability, not indefinite storage.
- Physical Android/macOS validation remains required before support labels
  advance beyond experimental.
