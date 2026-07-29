# ADR 0003: TLS-bound authenticated pairing

- Status: Accepted for experimental implementation
- Date: 2026-07-29
- Owners: Halo maintainers

## Context

Discovery returns attacker-controlled endpoint hints. Halo needs to connect to
those endpoints without an account or public-key infrastructure, let users
detect first-contact impersonation, and recognize the same cryptographic device
after an application restart.

The identity implementation should remain shared Rust code. Android Keystore
and macOS Keychain have incompatible signing APIs, so making them protocol
participants would duplicate algorithm, encoding, error, and lifecycle logic.
The TLS certificate used by QUIC also cannot itself be assumed to be the
long-lived device identity.

## Decision

### Framing and versioning

The control protocol uses a deterministic, length-delimited binary encoding.
Every frame has a magic value, wire version, message kind, flags, and bounded
payload length. Version 1 pairing messages have fixed field positions and
network byte order. Decoders reject non-zero reserved flags, unknown message
kinds, incorrect fixed lengths, trailing bytes, invalid ranges, and frames over
4096 bytes. Golden vectors are part of `halo-protocol`.

Breaking semantic or sequence changes require a new protocol version. Optional
future behavior uses an explicitly negotiated capability; it is not inferred
from ignored fields.

### Identities and TLS binding

Each installation owns a P-256 ECDSA identity key. Its public key uses the
65-byte SEC1 uncompressed representation. Rust generates the key and performs
all signing, verification, canonical encoding, and identity derivation.

The only platform storage interface saves, loads, and deletes an opaque secret
blob:

- Android: a non-exportable Android Keystore AES key wraps the Rust identity
  blob, which is stored in application-private storage and excluded from backup.
- iOS and macOS: an application-private Keychain generic-password item stores
  the Rust identity blob with a device-only accessibility class.

Platform code does not parse the blob, choose algorithms, sign protocol data,
derive peer identity, or decide trust. If protected storage is unavailable,
Halo reports that capability explicitly rather than writing the secret in
plaintext. Importing the blob into Rust memory is an accepted boundary: buffers
are short-lived and zeroized where the library permits, but process compromise
remains outside this design's protection.

Remembered-peer records contain public keys and policy metadata, not local
private key material. Rust may persist them in an atomically replaced private
file or database. Android SharedPreferences is not used for either store: it
adds a platform-specific format and weaker error/durability behavior without
solving private-key protection. A plaintext private-file implementation may be
provided only as an explicitly insecure development/test adapter.

An ephemeral QUIC TLS 1.3 certificate protects the connection. It is not a
trusted device credential. After TLS completes, both peers export 32 bytes using
the label `EXPORTER-Halo-Pairing-v1`. Each signed Hello covers the exporter,
role, supported/selected protocol version, capabilities, fresh 256-bit nonce,
and identity public key. The responder additionally signs the exact client
Hello digest. This binds the application identity and message order to this TLS
connection and prevents a recorded Hello from authenticating a new connection.

TLS 0-RTT is disabled for pairing and control messages. Pairing uses one ordered
bidirectional stream and a strict state machine.

### Short authentication code and trust commit

The transcript hash covers the exporter and exact encoded ClientHello and
ServerHello frames. HKDF-SHA-256 expands that hash under the context
`Halo pairing short code v1`. Rejection sampling maps the output to a six-digit
decimal value without modulo bias. Both peers display the same value.

Selecting a device is initiator consent. A new receiver must explicitly accept
after comparing the code. The receiver sends a signed decision bound to the
transcript; the initiator returns a signed commit. Neither side persists trust
before the accepted decision and commit validate. A reconnect may repeat the
ceremony if a crash occurs before both local stores finish; it must not silently
weaken authentication.

Trust records are keyed by a digest of the identity public key, contain the full
public key and negotiated protocol version, and are stored outside discovery
state. On later connections, valid signatures from the stored identity permit
automatic recognition. After a successful commit, Rust also binds the peer's
local-network IP address (excluding its restart-varying port) to that full trust
record in the app-private trust directory. If that address or an explicitly
claimed remembered peer presents a different key, the connection fails with
`IdentityChanged`; it never falls back to first-contact pairing in the same
attempt. IP reuse can therefore produce a conservative false positive. A peer
that moves to a new address is still recognized by its key after the handshake,
but cannot be preclassified as remembered from discovery metadata alone.

## Rejected alternatives

- Discovery identifiers, names, IP addresses, and BLE identifiers are not
  identities because attackers can copy or redirect them.
- A six-digit code derived only from public keys is replayable and does not bind
  the live TLS channel.
- Persisting an unprotected private-key file would enlarge backup and log
  exposure. The Rust-owned identity blob still requires OS-backed protection.
- Treating any self-signed TLS certificate as trusted without the signed,
  user-verified application handshake would permit undetectable interception.

## Initial Rust dependencies

- Quinn provides QUIC and its TLS exporter API; rustls is configured for TLS
  1.3, the `halo-pairing/1` ALPN, no 0-RTT, and bounded streams/timeouts.
- rcgen creates only the short-lived self-signed TLS certificate. That
  certificate is never persisted as or promoted to a Halo identity.
- RustCrypto P-256 implements fixed-width ECDSA identity signatures. HKDF and
  SHA-256 implement transcript hashing and short-code extraction.
- The implementations disable unused default features where practical. All are
  currently experimental dependencies under their upstream permissive licenses;
  security advisories, maintenance, binary size, and Android/iOS target support
  must be reviewed before a stable release.

## Consequences

First-contact pairing detects, rather than cryptographically prevents, an
active man-in-the-middle attacker: safety depends on users comparing the code
over an authentic visual context. Remembered peers get cryptographic rejection
of identity changes.

The crypto, identity-blob, and trust-store logic can be tested on every host;
native adapters remain byte-storage shims. Android/macOS support remains
experimental until those adapters and the complete foreground UI flow pass
physical-device tests.
