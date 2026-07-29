# Halo Pairing Protocol v1

- Status: Experimental
- Wire version: 1
- Maximum frame size: 4096 bytes, including the 12-byte header

This protocol authenticates a Halo device identity over an already established
QUIC/TLS 1.3 connection. Discovery identifiers and the TLS certificate are not
device identities. Pairing data must not be sent as 0-RTT data.

## Integer and frame encoding

All integers are unsigned and encoded in network byte order. All v1 messages
have fixed lengths. No Unicode strings, maps, implementation enum layouts, or
platform types occur on the wire.

| Offset | Size | Field | Validation |
| ---: | ---: | --- | --- |
| 0 | 4 | ASCII `HALO` | exact match |
| 4 | 2 | wire version | `1` |
| 6 | 1 | message kind | defined below |
| 7 | 1 | flags | zero in v1 |
| 8 | 4 | payload length | exact remaining bytes; total at most 4096 |
| 12 | variable | payload | exact length for kind |

Kinds are `1` ClientHello, `2` ServerHello, `3` PairingDecision, and `4`
PairingCommit. Unknown kinds, flags, lengths, and versions are fatal protocol
errors. A decoder must not allocate the declared payload before enforcing the
frame limit.

## Cryptographic fields

- Identity public key: 65-byte uncompressed SEC1 P-256 point.
- Signature: 64-byte P-256 ECDSA `r || s`, SHA-256 message digest, low-S form.
- Nonce: 32 random bytes from the operating system CSPRNG.
- Hash: SHA-256, 32 bytes.
- TLS channel binding: 32 bytes exported with TLS exporter label
  `EXPORTER-Halo-Pairing-v1` and empty context.

Signature inputs are domain-separated as described below. Length-prefix each
component with an unsigned 32-bit network-order length before hashing/signing.

## Messages

### ClientHello (kind 1, payload 173 bytes)

| Size | Field |
| ---: | --- |
| 2 | minimum supported application protocol version |
| 2 | maximum supported application protocol version |
| 8 | offered capability bits |
| 32 | client nonce |
| 65 | client identity public key |
| 64 | client signature |

The signature input components are domain `Halo ClientHello v1`, TLS exporter,
and the first 109 payload bytes.

### ServerHello (kind 2, payload 207 bytes)

| Size | Field |
| ---: | --- |
| 2 | selected application protocol version |
| 2 | minimum supported application protocol version |
| 2 | maximum supported application protocol version |
| 8 | server capability bits |
| 32 | server nonce |
| 65 | server identity public key |
| 32 | SHA-256 of the complete encoded ClientHello frame |
| 64 | server signature |

The selected version must be the highest version in the signed range
intersection. Negotiated capabilities are the intersection of the two signed
capability fields. The signature input components are domain
`Halo ServerHello v1`, TLS exporter, and the first 143 payload bytes.

### PairingDecision (kind 3, payload 97 bytes)

| Size | Field |
| ---: | --- |
| 32 | transcript hash |
| 1 | accepted: exactly 0 or 1 |
| 64 | server signature |

The signature input components are domain `Halo PairingDecision v1`, TLS
exporter, and the first 33 payload bytes.

### PairingCommit (kind 4, payload 96 bytes)

| Size | Field |
| ---: | --- |
| 32 | transcript hash |
| 64 | client signature |

The signature input components are domain `Halo PairingCommit v1`, TLS exporter,
and the first 32 payload bytes.

## Transcript and short code

The transcript hash is SHA-256 over length-prefixed components: domain
`Halo Pairing Transcript v1`, TLS exporter, exact ClientHello frame, and exact
ServerHello frame. The short-code input is that transcript hash.

HKDF-SHA-256 uses salt `Halo pairing short code v1` and info `decimal`. Read
successive 32-bit values from HKDF output and accept the first value below
`floor(2^32 / 1,000,000) * 1,000,000`; render its remainder modulo 1,000,000 as
six decimal digits. This rejection step avoids modulo bias.

Golden short-code vector: a 32-byte transcript hash containing `5a` in every
byte derives the display code `198987`.

## State sequence

```text
client                                      server
  |------------ ClientHello ----------------->|
  |<----------- ServerHello ------------------|
  |      both verify and display short code   |
  |<----------- PairingDecision --------------|
  |------------ PairingCommit ---------------->|
  |      both may persist the peer identity   |
```

For a remembered peer, both Hello signatures and the full stored public key
must match before automatic recognition. A changed identity is fatal and must
not be silently re-paired in the same connection.

Every message is valid only in its indicated state. A repeated, skipped, or
out-of-order message closes the control stream. A transcript mismatch,
signature failure, rejected decision, incompatible version, timeout,
cancellation, or connection loss must not create a trust record.
