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
