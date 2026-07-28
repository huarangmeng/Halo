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
  String get noticePermissionDenied =>
      'Required nearby-device, location, or local-network permission was denied.';

  @override
  String get noticeLocationServicesDisabled =>
      'Android location services are off, so this device may suppress BLE scan results. Turn on Location, then start discovery again.';

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
}
