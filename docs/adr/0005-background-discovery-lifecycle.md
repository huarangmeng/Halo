# ADR 0005: Discovery lifetime is independent of UI visibility

- Status: Accepted for Android and macOS experimental implementation
- Date: 2026-07-29

## Context

The demo originally stopped discovery whenever Flutter reported `paused`,
`hidden`, or `detached`. That coupled the Rust discovery session and native BLE
provider to window visibility. Minimizing the macOS window or putting Android
in the background therefore looked identical to an explicit user stop.

Users need an explicitly started session to continue while the application is
off-screen. This does not mean that Halo can survive force-stop, process exit,
device sleep, permission revocation, or every platform background policy.

## Decision

- Only the user-facing **Stop** operation ends a healthy session because of UI
  visibility. Flutter lifecycle transitions do not call `stop`.
- Android starts a `connectedDevice` foreground service with an ongoing
  notification for the duration of discovery. The service keeps the application
  process eligible to scan and advertise while the activity is off-screen.
- Android requires the runtime Bluetooth permissions and notification
  permission before starting this background-capable flow.
- macOS keeps the application process alive after the last window closes. The
  Rust and CoreBluetooth sessions continue while that process exists.
- iOS remains foreground-only and experimental. Halo does not claim unlimited
  CoreBluetooth, LAN discovery, or QUIC availability in the background.
- Platform capability diagnostics separately report Bluetooth, Wi-Fi, local
  network, and background-session state. A failed provider does not stop healthy
  providers.

## Consequences

- Android users receive persistent OS-visible disclosure while discovery is
  active and can return to Halo from the notification.
- Removing the foreground service or killing the process still ends discovery;
  automatic restoration is not implemented because restoring a Rust pairing
  identity and session without UI context needs a separate design.
- macOS window closure no longer implies application termination. A normal Quit
  still terminates discovery.
- Device tests must cover Home/minimize, screen lock, activity/window restore,
  explicit Stop, permission revocation, Bluetooth toggles, and OS process kill.

## Rejected alternatives

- Treating every Flutter pause as Stop was rejected because it made UI
  visibility an undocumented protocol lifecycle control.
- A silent Android background service was rejected because modern Android does
  not permit reliable long-running BLE work that way and users need visible
  disclosure.
- Claiming the same behavior on iOS was rejected because the OS controls BLE,
  network, and execution availability outside the foreground.
