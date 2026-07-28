package org.halo.discovery.android

import java.util.UUID

/** OS-facing identifiers only. Halo protocol bytes are owned and parsed by Rust. */
public object HaloBleUuids {
    public val SERVICE: UUID = UUID.fromString("b6882c7f-d426-4cb6-9012-d40bde5e2000")
    public val PRESENCE: UUID = UUID.fromString("8c2e5e61-4c6a-4c64-b804-1301a15251a0")
    public val WAKE_LAN: UUID = UUID.fromString("4fe6e851-dbc1-4a86-8e49-fcf1eabc1c82")
    public val ENDPOINT_HINTS: UUID = UUID.fromString("4672307b-caea-4e1a-8823-0bcea898ec83")
    internal val CLIENT_CHARACTERISTIC_CONFIGURATION: UUID =
        UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
}
