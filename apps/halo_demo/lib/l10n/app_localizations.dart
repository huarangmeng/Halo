import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:intl/intl.dart' as intl;

import 'app_localizations_en.dart';
import 'app_localizations_zh.dart';

// ignore_for_file: type=lint

/// Callers can lookup localized strings with an instance of AppLocalizations
/// returned by `AppLocalizations.of(context)`.
///
/// Applications need to include `AppLocalizations.delegate()` in their app's
/// `localizationDelegates` list, and the locales they support in the app's
/// `supportedLocales` list. For example:
///
/// ```dart
/// import 'l10n/app_localizations.dart';
///
/// return MaterialApp(
///   localizationsDelegates: AppLocalizations.localizationsDelegates,
///   supportedLocales: AppLocalizations.supportedLocales,
///   home: MyApplicationHome(),
/// );
/// ```
///
/// ## Update pubspec.yaml
///
/// Please make sure to update your pubspec.yaml to include the following
/// packages:
///
/// ```yaml
/// dependencies:
///   # Internationalization support.
///   flutter_localizations:
///     sdk: flutter
///   intl: any # Use the pinned version from flutter_localizations
///
///   # Rest of dependencies
/// ```
///
/// ## iOS Applications
///
/// iOS applications define key application metadata, including supported
/// locales, in an Info.plist file that is built into the application bundle.
/// To configure the locales supported by your app, you’ll need to edit this
/// file.
///
/// First, open your project’s ios/Runner.xcworkspace Xcode workspace file.
/// Then, in the Project Navigator, open the Info.plist file under the Runner
/// project’s Runner folder.
///
/// Next, select the Information Property List item, select Add Item from the
/// Editor menu, then select Localizations from the pop-up menu.
///
/// Select and expand the newly-created Localizations item then, for each
/// locale your application supports, add a new item and select the locale
/// you wish to add from the pop-up menu in the Value field. This list should
/// be consistent with the languages listed in the AppLocalizations.supportedLocales
/// property.
abstract class AppLocalizations {
  AppLocalizations(String locale)
    : localeName = intl.Intl.canonicalizedLocale(locale.toString());

  final String localeName;

  static AppLocalizations of(BuildContext context) {
    return Localizations.of<AppLocalizations>(context, AppLocalizations)!;
  }

  static const LocalizationsDelegate<AppLocalizations> delegate =
      _AppLocalizationsDelegate();

  /// A list of this localizations delegate along with the default localizations
  /// delegates.
  ///
  /// Returns a list of localizations delegates containing this delegate along with
  /// GlobalMaterialLocalizations.delegate, GlobalCupertinoLocalizations.delegate,
  /// and GlobalWidgetsLocalizations.delegate.
  ///
  /// Additional delegates can be added by appending to this list in
  /// MaterialApp. This list does not have to be used at all if a custom list
  /// of delegates is preferred or required.
  static const List<LocalizationsDelegate<dynamic>> localizationsDelegates =
      <LocalizationsDelegate<dynamic>>[
        delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
      ];

  /// A list of this localizations delegate's supported locales.
  static const List<Locale> supportedLocales = <Locale>[
    Locale('en'),
    Locale('zh'),
  ];

  /// No description provided for @appTitle.
  ///
  /// In en, this message translates to:
  /// **'Halo'**
  String get appTitle;

  /// No description provided for @appSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Nearby discovery · Flutter UI · Rust core · {platform}'**
  String appSubtitle(String platform);

  /// No description provided for @platformAndroid.
  ///
  /// In en, this message translates to:
  /// **'Android'**
  String get platformAndroid;

  /// No description provided for @platformIos.
  ///
  /// In en, this message translates to:
  /// **'iOS'**
  String get platformIos;

  /// No description provided for @platformMacos.
  ///
  /// In en, this message translates to:
  /// **'macOS'**
  String get platformMacos;

  /// No description provided for @platformWindows.
  ///
  /// In en, this message translates to:
  /// **'Windows'**
  String get platformWindows;

  /// No description provided for @platformLinux.
  ///
  /// In en, this message translates to:
  /// **'Linux'**
  String get platformLinux;

  /// No description provided for @deviceTypeUnknown.
  ///
  /// In en, this message translates to:
  /// **'Unknown'**
  String get deviceTypeUnknown;

  /// No description provided for @thisDevice.
  ///
  /// In en, this message translates to:
  /// **'This device'**
  String get thisDevice;

  /// No description provided for @discoverySessionId.
  ///
  /// In en, this message translates to:
  /// **'Discovery session ID'**
  String get discoverySessionId;

  /// No description provided for @discoverySessionIdPending.
  ///
  /// In en, this message translates to:
  /// **'Generated when discovery starts'**
  String get discoverySessionIdPending;

  /// No description provided for @deviceTypeLabel.
  ///
  /// In en, this message translates to:
  /// **'Device type'**
  String get deviceTypeLabel;

  /// No description provided for @peerIdLabel.
  ///
  /// In en, this message translates to:
  /// **'Peer ID'**
  String get peerIdLabel;

  /// No description provided for @discoverySourcesLabel.
  ///
  /// In en, this message translates to:
  /// **'Discovery sources'**
  String get discoverySourcesLabel;

  /// No description provided for @endpointLabel.
  ///
  /// In en, this message translates to:
  /// **'Endpoint'**
  String get endpointLabel;

  /// No description provided for @startDiscovery.
  ///
  /// In en, this message translates to:
  /// **'Start discovery'**
  String get startDiscovery;

  /// No description provided for @stop.
  ///
  /// In en, this message translates to:
  /// **'Stop'**
  String get stop;

  /// No description provided for @nearbyDevices.
  ///
  /// In en, this message translates to:
  /// **'Nearby devices ({count})'**
  String nearbyDevices(int count);

  /// No description provided for @nearbyHaloDevice.
  ///
  /// In en, this message translates to:
  /// **'Nearby Halo device'**
  String get nearbyHaloDevice;

  /// No description provided for @compatible.
  ///
  /// In en, this message translates to:
  /// **'Compatible'**
  String get compatible;

  /// No description provided for @incompatible.
  ///
  /// In en, this message translates to:
  /// **'Incompatible'**
  String get incompatible;

  /// No description provided for @bleAwaitingLan.
  ///
  /// In en, this message translates to:
  /// **'BLE rendezvous; awaiting LAN endpoint'**
  String get bleAwaitingLan;

  /// No description provided for @connectSecurely.
  ///
  /// In en, this message translates to:
  /// **'Connect securely'**
  String get connectSecurely;

  /// No description provided for @pairingIncomingTitle.
  ///
  /// In en, this message translates to:
  /// **'Pairing request'**
  String get pairingIncomingTitle;

  /// No description provided for @pairingCodeLabel.
  ///
  /// In en, this message translates to:
  /// **'Verify this code on both devices'**
  String get pairingCodeLabel;

  /// No description provided for @pairingFingerprintLabel.
  ///
  /// In en, this message translates to:
  /// **'Device key'**
  String get pairingFingerprintLabel;

  /// No description provided for @pairingConnecting.
  ///
  /// In en, this message translates to:
  /// **'Establishing an authenticated connection…'**
  String get pairingConnecting;

  /// No description provided for @pairingTrusted.
  ///
  /// In en, this message translates to:
  /// **'Trusted device'**
  String get pairingTrusted;

  /// No description provided for @pairingTrustedRecognized.
  ///
  /// In en, this message translates to:
  /// **'Previously trusted device recognized'**
  String get pairingTrustedRecognized;

  /// No description provided for @pairingRejected.
  ///
  /// In en, this message translates to:
  /// **'Pairing was rejected'**
  String get pairingRejected;

  /// No description provided for @pairingIdentityChanged.
  ///
  /// In en, this message translates to:
  /// **'Blocked: this device\'s identity changed'**
  String get pairingIdentityChanged;

  /// No description provided for @pairingTimedOut.
  ///
  /// In en, this message translates to:
  /// **'Pairing timed out'**
  String get pairingTimedOut;

  /// No description provided for @pairingFailed.
  ///
  /// In en, this message translates to:
  /// **'Secure pairing failed'**
  String get pairingFailed;

  /// No description provided for @pairingDisconnected.
  ///
  /// In en, this message translates to:
  /// **'Secure connection ended'**
  String get pairingDisconnected;

  /// No description provided for @pairingAccept.
  ///
  /// In en, this message translates to:
  /// **'Codes match — accept'**
  String get pairingAccept;

  /// No description provided for @pairingReject.
  ///
  /// In en, this message translates to:
  /// **'Reject'**
  String get pairingReject;

  /// No description provided for @connectionFailureTimeout.
  ///
  /// In en, this message translates to:
  /// **'Connection timed out before the peer responded.'**
  String get connectionFailureTimeout;

  /// No description provided for @connectionFailureUnreachable.
  ///
  /// In en, this message translates to:
  /// **'The discovered address is not reachable on the current network.'**
  String get connectionFailureUnreachable;

  /// No description provided for @connectionFailureTls.
  ///
  /// In en, this message translates to:
  /// **'The encrypted QUIC/TLS connection could not be established.'**
  String get connectionFailureTls;

  /// No description provided for @connectionFailureAuthentication.
  ///
  /// In en, this message translates to:
  /// **'The peer identity or handshake signature could not be verified.'**
  String get connectionFailureAuthentication;

  /// No description provided for @connectionFailureProtocol.
  ///
  /// In en, this message translates to:
  /// **'The peer sent an incompatible or malformed pairing message.'**
  String get connectionFailureProtocol;

  /// No description provided for @connectionFailureIdentityChanged.
  ///
  /// In en, this message translates to:
  /// **'The saved device identity does not match the identity presented now.'**
  String get connectionFailureIdentityChanged;

  /// No description provided for @connectionFailureNetworkChanged.
  ///
  /// In en, this message translates to:
  /// **'The active network changed while connecting.'**
  String get connectionFailureNetworkChanged;

  /// No description provided for @connectionFailureCancelled.
  ///
  /// In en, this message translates to:
  /// **'The connection attempt was cancelled.'**
  String get connectionFailureCancelled;

  /// No description provided for @connectionFailureConfiguration.
  ///
  /// In en, this message translates to:
  /// **'No usable connection endpoint or transport configuration is available.'**
  String get connectionFailureConfiguration;

  /// No description provided for @connectionFailureControlIo.
  ///
  /// In en, this message translates to:
  /// **'The authenticated control stream was interrupted.'**
  String get connectionFailureControlIo;

  /// No description provided for @connectionFailurePersistence.
  ///
  /// In en, this message translates to:
  /// **'The trusted-device record could not be saved safely.'**
  String get connectionFailurePersistence;

  /// No description provided for @connectionFailureUserInterface.
  ///
  /// In en, this message translates to:
  /// **'The pairing decision could not be completed by the app.'**
  String get connectionFailureUserInterface;

  /// No description provided for @connectionFailureInternal.
  ///
  /// In en, this message translates to:
  /// **'An internal connection task failed.'**
  String get connectionFailureInternal;

  /// No description provided for @connectionSessionClosed.
  ///
  /// In en, this message translates to:
  /// **'The authenticated transport closed. Reconnect to start a new authenticated session.'**
  String get connectionSessionClosed;

  /// No description provided for @connectionRetryRateLimited.
  ///
  /// In en, this message translates to:
  /// **'Wait briefly before retrying this device.'**
  String get connectionRetryRateLimited;

  /// No description provided for @connectionFailureUnknown.
  ///
  /// In en, this message translates to:
  /// **'Connection failed: {reason}'**
  String connectionFailureUnknown(String reason);

  /// No description provided for @emptyPeers.
  ///
  /// In en, this message translates to:
  /// **'Keep Halo open in the foreground on both devices.\nBLE and LAN discovery run in parallel after you start.'**
  String get emptyPeers;

  /// No description provided for @discoveryDiagnostics.
  ///
  /// In en, this message translates to:
  /// **'Discovery diagnostics'**
  String get discoveryDiagnostics;

  /// No description provided for @diagnosticsDescription.
  ///
  /// In en, this message translates to:
  /// **'Live provider health reported by the Rust discovery core. This data is local and intended for troubleshooting.'**
  String get diagnosticsDescription;

  /// No description provided for @diagnosticsCapabilities.
  ///
  /// In en, this message translates to:
  /// **'Device capabilities'**
  String get diagnosticsCapabilities;

  /// No description provided for @diagnosticsNoCapabilities.
  ///
  /// In en, this message translates to:
  /// **'Capability status is not available from this platform launcher.'**
  String get diagnosticsNoCapabilities;

  /// No description provided for @capabilityBluetooth.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth'**
  String get capabilityBluetooth;

  /// No description provided for @capabilityWifi.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi'**
  String get capabilityWifi;

  /// No description provided for @capabilityLocalNetwork.
  ///
  /// In en, this message translates to:
  /// **'Local network'**
  String get capabilityLocalNetwork;

  /// No description provided for @capabilityApplePeerToPeer.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi'**
  String get capabilityApplePeerToPeer;

  /// No description provided for @capabilityWifiDirect.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi Direct'**
  String get capabilityWifiDirect;

  /// No description provided for @capabilityWifiAware.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi Aware'**
  String get capabilityWifiAware;

  /// No description provided for @capabilityBackground.
  ///
  /// In en, this message translates to:
  /// **'Background discovery'**
  String get capabilityBackground;

  /// No description provided for @capabilityBluetoothReady.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth is on and BLE scanning/advertising are available.'**
  String get capabilityBluetoothReady;

  /// No description provided for @capabilityBluetoothOff.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth is turned off. Turn it on to discover over BLE.'**
  String get capabilityBluetoothOff;

  /// No description provided for @capabilityBluetoothPermissionRequired.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth permission has not been granted.'**
  String get capabilityBluetoothPermissionRequired;

  /// No description provided for @capabilityBluetoothPermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth access was denied in system privacy settings.'**
  String get capabilityBluetoothPermissionDenied;

  /// No description provided for @capabilityBluetoothUnsupported.
  ///
  /// In en, this message translates to:
  /// **'This device does not support the required BLE features.'**
  String get capabilityBluetoothUnsupported;

  /// No description provided for @capabilityBluetoothAdvertisingUnavailable.
  ///
  /// In en, this message translates to:
  /// **'BLE scanning is available, but this device cannot advertise.'**
  String get capabilityBluetoothAdvertisingUnavailable;

  /// No description provided for @capabilityBluetoothDegraded.
  ///
  /// In en, this message translates to:
  /// **'A BLE scan, GATT, or advertising operation failed; see recent events.'**
  String get capabilityBluetoothDegraded;

  /// No description provided for @capabilityBluetoothResetting.
  ///
  /// In en, this message translates to:
  /// **'The system Bluetooth stack is resetting.'**
  String get capabilityBluetoothResetting;

  /// No description provided for @capabilityBluetoothPending.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth power state will be checked when discovery starts.'**
  String get capabilityBluetoothPending;

  /// No description provided for @capabilityWifiConnected.
  ///
  /// In en, this message translates to:
  /// **'Connected to Wi-Fi.'**
  String get capabilityWifiConnected;

  /// No description provided for @capabilityWifiOff.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi is turned off.'**
  String get capabilityWifiOff;

  /// No description provided for @capabilityWifiNotConnected.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi is on but not connected to a network.'**
  String get capabilityWifiNotConnected;

  /// No description provided for @capabilityWifiUnsupported.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi status is unavailable on this device.'**
  String get capabilityWifiUnsupported;

  /// No description provided for @capabilityLocalNetworkConnected.
  ///
  /// In en, this message translates to:
  /// **'A local-network route is available.'**
  String get capabilityLocalNetworkConnected;

  /// No description provided for @capabilityLocalNetworkSocketBound.
  ///
  /// In en, this message translates to:
  /// **'QUIC is pinned to the current unmetered local network.'**
  String get capabilityLocalNetworkSocketBound;

  /// No description provided for @capabilityLocalNetworkMetered.
  ///
  /// In en, this message translates to:
  /// **'The current local network is metered; nearby mode will not use it for transfer.'**
  String get capabilityLocalNetworkMetered;

  /// No description provided for @capabilityLocalNetworkConstrained.
  ///
  /// In en, this message translates to:
  /// **'The current local network is in Low Data Mode; nearby mode will not use it for transfer.'**
  String get capabilityLocalNetworkConstrained;

  /// No description provided for @capabilityLocalNetworkVpn.
  ///
  /// In en, this message translates to:
  /// **'The active local route includes a VPN; nearby mode will not use it.'**
  String get capabilityLocalNetworkVpn;

  /// No description provided for @capabilityLocalNetworkBindingFailed.
  ///
  /// In en, this message translates to:
  /// **'The local-network socket could not be pinned safely.'**
  String get capabilityLocalNetworkBindingFailed;

  /// No description provided for @capabilityLocalNetworkNotPrepared.
  ///
  /// In en, this message translates to:
  /// **'The local-network socket has not been prepared yet.'**
  String get capabilityLocalNetworkNotPrepared;

  /// No description provided for @capabilityLocalNetworkRestartRequired.
  ///
  /// In en, this message translates to:
  /// **'The local network changed; restart discovery to bind a new QUIC socket.'**
  String get capabilityLocalNetworkRestartRequired;

  /// No description provided for @capabilityEthernetConnected.
  ///
  /// In en, this message translates to:
  /// **'A local-network route is available over Ethernet.'**
  String get capabilityEthernetConnected;

  /// No description provided for @capabilityNoLocalNetwork.
  ///
  /// In en, this message translates to:
  /// **'No Wi-Fi or Ethernet local-network route is available; QUIC pairing cannot connect.'**
  String get capabilityNoLocalNetwork;

  /// No description provided for @capabilityLocalNetworkPermissionRequired.
  ///
  /// In en, this message translates to:
  /// **'Local-network permission has not been granted.'**
  String get capabilityLocalNetworkPermissionRequired;

  /// No description provided for @capabilityAppleP2PStarting.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi is starting.'**
  String get capabilityAppleP2PStarting;

  /// No description provided for @capabilityAppleP2PReady.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi is ready and restricted to non-cellular Wi-Fi paths.'**
  String get capabilityAppleP2PReady;

  /// No description provided for @capabilityAppleP2PUnavailable.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi is temporarily unavailable on this device or network state.'**
  String get capabilityAppleP2PUnavailable;

  /// No description provided for @capabilityAppleP2PFailed.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi failed; LAN discovery and transfer remain available.'**
  String get capabilityAppleP2PFailed;

  /// No description provided for @capabilityAppleP2PStopped.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi is stopped.'**
  String get capabilityAppleP2PStopped;

  /// No description provided for @capabilityAppleP2PIdentityFailed.
  ///
  /// In en, this message translates to:
  /// **'Apple peer-to-peer Wi-Fi could not create its temporary encrypted transport identity.'**
  String get capabilityAppleP2PIdentityFailed;

  /// No description provided for @capabilityDirectProviderPending.
  ///
  /// In en, this message translates to:
  /// **'Platform support was detected, but the Halo provider is not implemented yet.'**
  String get capabilityDirectProviderPending;

  /// No description provided for @capabilityDirectUnsupported.
  ///
  /// In en, this message translates to:
  /// **'This platform or device does not expose this direct channel.'**
  String get capabilityDirectUnsupported;

  /// No description provided for @capabilityWifiAwareUnavailable.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi Aware is currently unavailable.'**
  String get capabilityWifiAwareUnavailable;

  /// No description provided for @capabilityWifiAwarePermissionRequired.
  ///
  /// In en, this message translates to:
  /// **'Wi-Fi Aware permission or entitlement is required.'**
  String get capabilityWifiAwarePermissionRequired;

  /// No description provided for @capabilityBackgroundRunning.
  ///
  /// In en, this message translates to:
  /// **'Android foreground service is keeping discovery active off-screen.'**
  String get capabilityBackgroundRunning;

  /// No description provided for @capabilityBackgroundStopped.
  ///
  /// In en, this message translates to:
  /// **'Background discovery service is stopped.'**
  String get capabilityBackgroundStopped;

  /// No description provided for @capabilityBackgroundProcess.
  ///
  /// In en, this message translates to:
  /// **'Discovery continues while the macOS application process is running.'**
  String get capabilityBackgroundProcess;

  /// No description provided for @capabilityForegroundOnly.
  ///
  /// In en, this message translates to:
  /// **'This platform only supports the current foreground discovery flow.'**
  String get capabilityForegroundOnly;

  /// No description provided for @diagnosticsSessionState.
  ///
  /// In en, this message translates to:
  /// **'Session state'**
  String get diagnosticsSessionState;

  /// No description provided for @diagnosticsProviders.
  ///
  /// In en, this message translates to:
  /// **'Provider health'**
  String get diagnosticsProviders;

  /// No description provided for @diagnosticsNoProviders.
  ///
  /// In en, this message translates to:
  /// **'Start discovery to inspect provider health.'**
  String get diagnosticsNoProviders;

  /// No description provided for @diagnosticsRecentEvents.
  ///
  /// In en, this message translates to:
  /// **'Recent native BLE events'**
  String get diagnosticsRecentEvents;

  /// No description provided for @diagnosticsNoEvents.
  ///
  /// In en, this message translates to:
  /// **'No native BLE errors have been reported.'**
  String get diagnosticsNoEvents;

  /// No description provided for @discoveryStatusSemantics.
  ///
  /// In en, this message translates to:
  /// **'Discovery status: {status}'**
  String discoveryStatusSemantics(String status);

  /// No description provided for @statusStopped.
  ///
  /// In en, this message translates to:
  /// **'Stopped'**
  String get statusStopped;

  /// No description provided for @statusPreparing.
  ///
  /// In en, this message translates to:
  /// **'Waiting for permission'**
  String get statusPreparing;

  /// No description provided for @statusStarting.
  ///
  /// In en, this message translates to:
  /// **'Starting all providers'**
  String get statusStarting;

  /// No description provided for @statusRunning.
  ///
  /// In en, this message translates to:
  /// **'Discovering nearby devices'**
  String get statusRunning;

  /// No description provided for @statusDegraded.
  ///
  /// In en, this message translates to:
  /// **'Discovery partially available'**
  String get statusDegraded;

  /// No description provided for @statusFailed.
  ///
  /// In en, this message translates to:
  /// **'Discovery needs attention'**
  String get statusFailed;

  /// No description provided for @noticeStopped.
  ///
  /// In en, this message translates to:
  /// **'Discovery is stopped. No radio or network work is running.'**
  String get noticeStopped;

  /// No description provided for @noticePermissionContext.
  ///
  /// In en, this message translates to:
  /// **'Some Android devices require Nearby devices and precise location before returning BLE scan results. Halo does not derive, store, or transmit your location.'**
  String get noticePermissionContext;

  /// No description provided for @noticeApplePermissionContext.
  ///
  /// In en, this message translates to:
  /// **'Halo needs Bluetooth and local-network access while open to discover nearby devices. Discovery metadata stays on your local links.'**
  String get noticeApplePermissionContext;

  /// No description provided for @noticePermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'Required nearby-device, location, or local-network permission was denied.'**
  String get noticePermissionDenied;

  /// No description provided for @noticeLocationServicesDisabled.
  ///
  /// In en, this message translates to:
  /// **'Android location services are off, so this device may suppress BLE scan results. Turn on Location, then start discovery again.'**
  String get noticeLocationServicesDisabled;

  /// No description provided for @noticeIosBluetoothPermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'iOS blocked Bluetooth for Halo. Enable Bluetooth access for Halo in Settings > Privacy & Security > Bluetooth, then start discovery again.'**
  String get noticeIosBluetoothPermissionDenied;

  /// No description provided for @noticeMacosBluetoothPermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'macOS blocked Bluetooth for Halo. Enable Halo in System Settings > Privacy & Security > Bluetooth, then fully restart the app. Ad-hoc debug rebuilds may require a new grant.'**
  String get noticeMacosBluetoothPermissionDenied;

  /// No description provided for @noticeStarting.
  ///
  /// In en, this message translates to:
  /// **'Rust is starting BLE, mDNS, IPv4 and IPv6 discovery.'**
  String get noticeStarting;

  /// No description provided for @noticeRunning.
  ///
  /// In en, this message translates to:
  /// **'BLE and Rust LAN providers are running in parallel.'**
  String get noticeRunning;

  /// No description provided for @noticeNativeEventStopped.
  ///
  /// In en, this message translates to:
  /// **'The platform BLE event stream stopped: {detail}'**
  String noticeNativeEventStopped(String detail);

  /// No description provided for @noticeStartFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not start discovery: {detail}'**
  String noticeStartFailed(String detail);

  /// No description provided for @noticeCleanupFailed.
  ///
  /// In en, this message translates to:
  /// **'Discovery stopped with a cleanup error: {detail}'**
  String noticeCleanupFailed(String detail);

  /// No description provided for @noticeBleUnavailable.
  ///
  /// In en, this message translates to:
  /// **'BLE is {state}; LAN providers remain independent.'**
  String noticeBleUnavailable(String state);

  /// No description provided for @noticeCapabilityHealthDegraded.
  ///
  /// In en, this message translates to:
  /// **'Device capabilities need attention ({capabilities}); available discovery paths keep running.'**
  String noticeCapabilityHealthDegraded(String capabilities);

  /// No description provided for @noticeProviderHealthDegraded.
  ///
  /// In en, this message translates to:
  /// **'Some providers need attention ({providers}); healthy providers keep running.'**
  String noticeProviderHealthDegraded(String providers);

  /// No description provided for @noticeDiagnostic.
  ///
  /// In en, this message translates to:
  /// **'{operation}: {detail}'**
  String noticeDiagnostic(String operation, String detail);

  /// No description provided for @noticeRustRejected.
  ///
  /// In en, this message translates to:
  /// **'Rust rejected a native discovery event: {detail}'**
  String noticeRustRejected(String detail);

  /// No description provided for @providerStarting.
  ///
  /// In en, this message translates to:
  /// **'starting'**
  String get providerStarting;

  /// No description provided for @providerReady.
  ///
  /// In en, this message translates to:
  /// **'ready'**
  String get providerReady;

  /// No description provided for @providerPermissionRequired.
  ///
  /// In en, this message translates to:
  /// **'waiting for permission'**
  String get providerPermissionRequired;

  /// No description provided for @providerPermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'permission denied'**
  String get providerPermissionDenied;

  /// No description provided for @providerHardwareOff.
  ///
  /// In en, this message translates to:
  /// **'powered off'**
  String get providerHardwareOff;

  /// No description provided for @providerUnsupported.
  ///
  /// In en, this message translates to:
  /// **'unsupported'**
  String get providerUnsupported;

  /// No description provided for @providerTemporarilyUnavailable.
  ///
  /// In en, this message translates to:
  /// **'temporarily unavailable'**
  String get providerTemporarilyUnavailable;

  /// No description provided for @providerStopped.
  ///
  /// In en, this message translates to:
  /// **'stopped'**
  String get providerStopped;

  /// No description provided for @providerDegraded.
  ///
  /// In en, this message translates to:
  /// **'degraded'**
  String get providerDegraded;

  /// No description provided for @providerFailedRecoverable.
  ///
  /// In en, this message translates to:
  /// **'failed; retry available'**
  String get providerFailedRecoverable;

  /// No description provided for @providerFailed.
  ///
  /// In en, this message translates to:
  /// **'failed'**
  String get providerFailed;

  /// No description provided for @providerPresenceV4.
  ///
  /// In en, this message translates to:
  /// **'IPv4 Presence'**
  String get providerPresenceV4;

  /// No description provided for @providerPresenceV6.
  ///
  /// In en, this message translates to:
  /// **'IPv6 Presence'**
  String get providerPresenceV6;

  /// No description provided for @transferIncomingTitle.
  ///
  /// In en, this message translates to:
  /// **'Incoming file'**
  String get transferIncomingTitle;

  /// No description provided for @transferOfferDescription.
  ///
  /// In en, this message translates to:
  /// **'{name} · {size}'**
  String transferOfferDescription(String name, String size);

  /// No description provided for @transferAccept.
  ///
  /// In en, this message translates to:
  /// **'Accept file'**
  String get transferAccept;

  /// No description provided for @transferReject.
  ///
  /// In en, this message translates to:
  /// **'Reject'**
  String get transferReject;

  /// No description provided for @transferSendFile.
  ///
  /// In en, this message translates to:
  /// **'Send file'**
  String get transferSendFile;

  /// No description provided for @transferLanOnly.
  ///
  /// In en, this message translates to:
  /// **'Files use the authenticated local QUIC connection; BLE carries no file bytes.'**
  String get transferLanOnly;

  /// No description provided for @transferAwaitingDecision.
  ///
  /// In en, this message translates to:
  /// **'Waiting for the receiver'**
  String get transferAwaitingDecision;

  /// No description provided for @transferTransferring.
  ///
  /// In en, this message translates to:
  /// **'Transferring'**
  String get transferTransferring;

  /// No description provided for @transferCompleted.
  ///
  /// In en, this message translates to:
  /// **'Transfer complete'**
  String get transferCompleted;

  /// No description provided for @transferRejected.
  ///
  /// In en, this message translates to:
  /// **'Transfer rejected'**
  String get transferRejected;

  /// No description provided for @transferCancelled.
  ///
  /// In en, this message translates to:
  /// **'Transfer cancelled'**
  String get transferCancelled;

  /// No description provided for @transferFailed.
  ///
  /// In en, this message translates to:
  /// **'Transfer failed'**
  String get transferFailed;

  /// No description provided for @transferCancel.
  ///
  /// In en, this message translates to:
  /// **'Cancel transfer'**
  String get transferCancel;

  /// No description provided for @transferReceivedAt.
  ///
  /// In en, this message translates to:
  /// **'Saved in private app storage: {path}'**
  String transferReceivedAt(String path);
}

class _AppLocalizationsDelegate
    extends LocalizationsDelegate<AppLocalizations> {
  const _AppLocalizationsDelegate();

  @override
  Future<AppLocalizations> load(Locale locale) {
    return SynchronousFuture<AppLocalizations>(lookupAppLocalizations(locale));
  }

  @override
  bool isSupported(Locale locale) =>
      <String>['en', 'zh'].contains(locale.languageCode);

  @override
  bool shouldReload(_AppLocalizationsDelegate old) => false;
}

AppLocalizations lookupAppLocalizations(Locale locale) {
  // Lookup logic when only language code is specified.
  switch (locale.languageCode) {
    case 'en':
      return AppLocalizationsEn();
    case 'zh':
      return AppLocalizationsZh();
  }

  throw FlutterError(
    'AppLocalizations.delegate failed to load unsupported locale "$locale". This is likely '
    'an issue with the localizations generation tool. Please file an issue '
    'on GitHub with a reproducible sample app and the gen-l10n configuration '
    'that was used.',
  );
}
