# Halo BLE Rendezvous v1

Status: Experimental

BLE provides proximity rendezvous and can trigger LAN probing. It does not
authenticate a device and never carries file data.

## UUIDs

| Item | UUID |
| --- | --- |
| Halo service | `b6882c7f-d426-4cb6-9012-d40bde5e2000` |
| Presence characteristic | `8c2e5e61-4c6a-4c64-b804-1301a15251a0` |
| Wake LAN characteristic | `4fe6e851-dbc1-4a86-8e49-fcf1eabc1c82` |
| Endpoint hints characteristic | `4672307b-caea-4e1a-8823-0bcea898ec83` |

These are project-owned 128-bit UUIDs. Halo does not use an unassigned Bluetooth
SIG company identifier.

## Advertisement

The portable advertisement contract contains the Halo service UUID. Platform-
specific service data may include a rotating truncated presence token and
protocol major version, but scanners cannot require that extension.

## Presence characteristic

- Properties: Read, Notify
- Value: exactly one 58-byte Halo Presence Protocol v1 `announce` packet
- Maximum logical value: 58 bytes; the platform BLE stack may fragment it
- Security meaning: untrusted rendezvous hint only

Using the same fixed codec as LAN Presence prevents four native adapters from
inventing incompatible descriptor formats. A native adapter validates the value
with `halo_discovery::ble::decode_presence` before submitting an observation.

## Wake LAN characteristic

- Properties: Write, Notify
- Request value: an 8-byte random nonce
- Behavior: the GATT server asks active LAN providers to issue an immediate
  query/announce cycle
- Notification value: the same nonce followed by a one-byte status

Status values:

| Value | Meaning |
| ---: | --- |
| `0` | LAN probe scheduled |
| `1` | no LAN provider currently available |
| `2` | request rate-limited |
| `3` | malformed request |

The server must rate-limit writes per temporary peripheral/central and globally.

## Endpoint hints characteristic

This optional characteristic is not yet frozen. It will carry a bounded list of
IP address, interface scope, transport, and port hints. Every hint remains
untrusted and must complete the normal secure handshake. Native adapters must
not ship an ad-hoc encoding before this section is finalized with golden vectors.
