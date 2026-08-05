# Halo Presence Protocol v1

Status: Experimental

This protocol provides untrusted LAN rendezvous hints. It does not authenticate
a peer and must never be used as proof of identity or authorization.

## Transport

- IPv4 group: `239.192.72.65`
- IPv6 group: `ff12::4841:4c4f` (transient link-local scope)
- UDP port: `44721`
- IPv4 multicast TTL / IPv6 hop limit: `1`
- Maximum accepted datagram: exactly `58` bytes in v1
- Integer encoding: unsigned, network byte order (big endian)

The IPv4 group is in the organization-local administratively scoped range. The
same packet is also sent to each eligible IPv4 interface's directed-broadcast
address. IPv6 group membership and sending are scoped per interface; a received
link-local endpoint is invalid without its interface scope.

## Packet

| Offset | Size | Field | Constraint |
| ---: | ---: | --- | --- |
| 0 | 8 | magic | ASCII `HALODSC1` |
| 8 | 1 | wire version | `1` |
| 9 | 1 | kind | `1=query`, `2=response`, `3=announce`, `4=goodbye` |
| 10 | 2 | reply port | non-zero for query; zero for every other kind |
| 12 | 16 | presence ID | random for this application presence |
| 28 | 2 | QUIC port | non-zero |
| 30 | 2 | minimum protocol version | non-zero and `min <= max` |
| 32 | 2 | maximum protocol version | non-zero and `min <= max` |
| 34 | 8 | capability and device metadata bits | unknown bits ignored during discovery |
| 42 | 8 | sequence | monotonically increasing per presence |
| 50 | 8 | nonce | query nonce, copied into its response |

Receivers reject packets of any other length, unknown kind, invalid reply-port
semantics, unsupported wire version, invalid QUIC port, or invalid protocol range.

### Capability and device metadata bits

Bits 60–63 (the most-significant nibble) encode an optional, untrusted coarse
device type. `0=unknown`, `1=Android`, `2=iOS`, `3=macOS`, `4=Windows`, and
`5=Linux`; values `6–15` are reserved and display as unknown. Bits 0–59 remain
feature capabilities. A sender may leave the device type unset, and an older
receiver safely ignores this additive metadata.

Device type is UI metadata only. It must not influence trust, pairing,
authorization, or transport security decisions.

## Behavior

- A node sends `query` immediately at startup and periodically sends `announce`.
- A node receiving `query` records the sender as an observation and replies by
  unicast to the packet source IP and declared reply port with `response`,
  copying the query nonce.
- A matching response supplies a round-trip-time sample.
- A node should send `goodbye` during an orderly shutdown. Receivers cannot rely
  on goodbye and must expire all observations by TTL.
- Source IP plus the advertised QUIC port forms the candidate endpoint. No IP
  address is carried inside the packet.
- Transfer v1 accepts only loopback (tests), RFC 1918, IPv4 link-local, IPv6
  link-local, and IPv6 unique-local candidates. A global address is rejected
  until Halo can bind it to the LAN interface that produced the observation;
  cellular and public-network fallback is not part of the protocol.
- Implementations ignore their own `PresenceId`.

All fields are attacker-controlled until the later secure transport handshake
binds the endpoint to a cryptographic device identity.
