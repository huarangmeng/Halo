// ignore: unused_import
import 'package:intl/intl.dart' as intl;
import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for English (`en`).
class AppLocalizationsEn extends AppLocalizations {
  AppLocalizationsEn([String locale = 'en']) : super(locale);

  @override
  String get appTitle => 'Halo';

  @override
  String appSubtitle(String platform) {
    return 'Nearby discovery · Flutter UI · Rust core · $platform';
  }

  @override
  String get platformAndroid => 'Android';

  @override
  String get platformIos => 'iOS';

  @override
  String get platformMacos => 'macOS';

  @override
  String get platformWindows => 'Windows';

  @override
  String get platformLinux => 'Linux';

  @override
  String get deviceTypeUnknown => 'Unknown';

  @override
  String get thisDevice => 'This device';

  @override
  String get discoverySessionId => 'Discovery session ID';

  @override
  String get discoverySessionIdPending => 'Generated when discovery starts';

  @override
  String get deviceTypeLabel => 'Device type';

  @override
  String get peerIdLabel => 'Peer ID';

  @override
  String get discoverySourcesLabel => 'Discovery sources';

  @override
  String get endpointLabel => 'Endpoint';

  @override
  String get startDiscovery => 'Start discovery';

  @override
  String get stop => 'Stop';

  @override
  String nearbyDevices(int count) {
    return 'Nearby devices ($count)';
  }

  @override
  String get nearbyHaloDevice => 'Nearby Halo device';

  @override
  String get compatible => 'Compatible';

  @override
  String get incompatible => 'Incompatible';

  @override
  String get bleAwaitingLan => 'BLE rendezvous; awaiting LAN endpoint';

  @override
  String get emptyPeers =>
      'Keep Halo open in the foreground on both devices.\nBLE and LAN discovery run in parallel after you start.';

  @override
  String get discoveryDiagnostics => 'Discovery diagnostics';

  @override
  String get diagnosticsDescription =>
      'Live provider health reported by the Rust discovery core. This data is local and intended for troubleshooting.';

  @override
  String get diagnosticsSessionState => 'Session state';

  @override
  String get diagnosticsProviders => 'Provider health';

  @override
  String get diagnosticsNoProviders =>
      'Start discovery to inspect provider health.';

  @override
  String get diagnosticsRecentEvents => 'Recent native BLE events';

  @override
  String get diagnosticsNoEvents => 'No native BLE errors have been reported.';

  @override
  String discoveryStatusSemantics(String status) {
    return 'Discovery status: $status';
  }

  @override
  String get statusStopped => 'Stopped';

  @override
  String get statusPreparing => 'Waiting for permission';

  @override
  String get statusStarting => 'Starting all providers';

  @override
  String get statusRunning => 'Discovering nearby devices';

  @override
  String get statusDegraded => 'Discovery partially available';

  @override
  String get statusFailed => 'Discovery needs attention';

  @override
  String get noticeStopped =>
      'Discovery is stopped. No radio or network work is running.';

  @override
  String get noticePermissionContext =>
      'Some Android devices require Nearby devices and precise location before returning BLE scan results. Halo does not derive, store, or transmit your location.';

  @override
  String get noticeApplePermissionContext =>
      'Halo needs Bluetooth and local-network access while open to discover nearby devices. Discovery metadata stays on your local links.';

  @override
  String get noticePermissionDenied =>
      'Required nearby-device, location, or local-network permission was denied.';

  @override
  String get noticeLocationServicesDisabled =>
      'Android location services are off, so this device may suppress BLE scan results. Turn on Location, then start discovery again.';

  @override
  String get noticeIosBluetoothPermissionDenied =>
      'iOS blocked Bluetooth for Halo. Enable Bluetooth access for Halo in Settings > Privacy & Security > Bluetooth, then start discovery again.';

  @override
  String get noticeMacosBluetoothPermissionDenied =>
      'macOS blocked Bluetooth for Halo. Enable Halo in System Settings > Privacy & Security > Bluetooth, then fully restart the app. Ad-hoc debug rebuilds may require a new grant.';

  @override
  String get noticeStarting =>
      'Rust is starting BLE, mDNS, IPv4 and IPv6 discovery.';

  @override
  String get noticeRunning =>
      'BLE and Rust LAN providers are running in parallel.';

  @override
  String noticeNativeEventStopped(String detail) {
    return 'The platform BLE event stream stopped: $detail';
  }

  @override
  String noticeStartFailed(String detail) {
    return 'Could not start discovery: $detail';
  }

  @override
  String noticeCleanupFailed(String detail) {
    return 'Discovery stopped with a cleanup error: $detail';
  }

  @override
  String noticeBleUnavailable(String state) {
    return 'BLE is $state; LAN providers remain independent.';
  }

  @override
  String noticeProviderHealthDegraded(String providers) {
    return 'Some providers need attention ($providers); healthy providers keep running.';
  }

  @override
  String noticeDiagnostic(String operation, String detail) {
    return '$operation: $detail';
  }

  @override
  String noticeRustRejected(String detail) {
    return 'Rust rejected a native discovery event: $detail';
  }

  @override
  String get providerStarting => 'starting';

  @override
  String get providerReady => 'ready';

  @override
  String get providerPermissionRequired => 'waiting for permission';

  @override
  String get providerPermissionDenied => 'permission denied';

  @override
  String get providerHardwareOff => 'powered off';

  @override
  String get providerUnsupported => 'unsupported';

  @override
  String get providerTemporarilyUnavailable => 'temporarily unavailable';

  @override
  String get providerStopped => 'stopped';

  @override
  String get providerDegraded => 'degraded';

  @override
  String get providerFailedRecoverable => 'failed; retry available';

  @override
  String get providerFailed => 'failed';

  @override
  String get providerPresenceV4 => 'IPv4 Presence';

  @override
  String get providerPresenceV6 => 'IPv6 Presence';
}
