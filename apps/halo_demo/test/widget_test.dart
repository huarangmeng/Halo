import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show Uint64List;
import 'package:halo_demo/discovery_controller.dart';
import 'package:halo_demo/l10n/app_localizations.dart';
import 'package:halo_demo/main.dart';
import 'package:halo_demo/src/rust/api.dart';
import 'package:halo_demo/src/rust/api/pairing_api.dart';

void main() {
  test('LAN endpoint is connectable only while the platform path is ready', () {
    final controller = DiscoveryController(platformOverride: 'android');
    addTearDown(controller.dispose);
    final peer = DiscoveryPeer(
      presenceId: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
      deviceType: DiscoveryDeviceType.macos,
      compatible: true,
      capabilities: BigInt.zero,
      sources: const ['mdns'],
      candidateEndpoints: const ['192.0.2.1:4433'],
      candidateCount: 1,
      quarantined: false,
    );

    controller.platformCapabilities = const [
      PlatformCapabilityStatus(
        name: 'local_network',
        state: 'temporarily_unavailable',
        detail: 'no_local_network_route',
      ),
    ];
    expect(controller.hasConnectablePathFor(peer), isFalse);

    controller.platformCapabilities = const [
      PlatformCapabilityStatus(
        name: 'local_network',
        state: 'ready',
        detail: 'local_network_connected',
      ),
    ];
    expect(controller.hasConnectablePathFor(peer), isTrue);
  });

  testWidgets('renders the shared discovery screen in English', (tester) async {
    await tester.pumpWidget(const HaloApp());

    expect(find.text('Halo'), findsOneWidget);
    expect(find.text('Start discovery'), findsOneWidget);
    expect(find.text('Stopped'), findsOneWidget);
    expect(find.textContaining('Flutter UI · Rust core'), findsOneWidget);
  });

  testWidgets('follows a Simplified Chinese system locale', (tester) async {
    tester.platformDispatcher.localesTestValue = const [Locale('zh', 'CN')];
    addTearDown(tester.platformDispatcher.clearLocalesTestValue);

    await tester.pumpWidget(const HaloApp());
    await tester.pumpAndSettle();

    expect(find.text('开始发现'), findsOneWidget);
    expect(find.text('已停止'), findsOneWidget);
    expect(find.textContaining('Flutter 界面 · Rust 核心'), findsOneWidget);
  });

  testWidgets('shows complete local and peer IDs with device types', (
    tester,
  ) async {
    const localId = '11111111-2222-4333-8444-555555555555';
    const peerId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
    final controller = DiscoveryController()
      ..localPresenceId = localId
      ..localDeviceType = DiscoveryDeviceType.macos
      ..peers = [
        DiscoveryPeer(
          presenceId: peerId,
          deviceType: DiscoveryDeviceType.android,
          compatible: true,
          capabilities: BigInt.zero,
          sources: ['ble-macos'],
          candidateEndpoints: const [],
          candidateCount: 0,
          quarantined: false,
        ),
      ];
    addTearDown(controller.dispose);
    await tester.binding.setSurfaceSize(const Size(1000, 1200));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text(localId), findsOneWidget);
    expect(find.text(peerId), findsOneWidget);
    expect(find.text('macOS'), findsOneWidget);
    expect(
      find.textContaining('Device type: Android', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('aaaaaaaa…'), findsNothing);
  });

  testWidgets('renders iOS and Rust provider diagnostics', (tester) async {
    final controller = DiscoveryController(platformOverride: 'ios')
      ..platformCapabilities = const [
        PlatformCapabilityStatus(
          name: 'bluetooth',
          state: 'hardware_off',
          detail: 'bluetooth_powered_off',
        ),
        PlatformCapabilityStatus(
          name: 'local_network',
          state: 'temporarily_unavailable',
          detail: 'no_local_network_route',
        ),
        PlatformCapabilityStatus(
          name: 'wifi_aware',
          state: 'stopped',
          detail: 'wifi_aware_provider_not_implemented',
        ),
      ]
      ..providerStatuses = const [
        DiscoveryProviderStatus(
          name: 'ble-ios',
          kind: 'ble',
          state: 'permission_denied',
          detail: 'denied by user',
        ),
        DiscoveryProviderStatus(name: 'mdns', kind: 'mdns', state: 'ready'),
      ]
      ..diagnostics = const [
        DiscoveryDiagnosticEntry(
          operation: 'corebluetooth',
          detail: 'advertising failed',
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.textContaining('Rust core · iOS'), findsOneWidget);
    await tester.tap(find.byTooltip('Discovery diagnostics'));
    await tester.pumpAndSettle();

    expect(find.text('Bluetooth'), findsOneWidget);
    expect(find.textContaining('Bluetooth is turned off'), findsOneWidget);
    expect(find.text('Local network'), findsOneWidget);
    expect(find.textContaining('QUIC pairing cannot connect'), findsOneWidget);
    expect(find.text('Wi-Fi Aware'), findsOneWidget);
    expect(find.textContaining('provider is not implemented'), findsOneWidget);
    await tester.scrollUntilVisible(
      find.text('mdns'),
      300,
      scrollable: find.byType(Scrollable).last,
    );
    expect(find.text('ble-ios'), findsOneWidget);
    expect(find.text('mdns'), findsOneWidget);
    expect(find.textContaining('permission denied'), findsOneWidget);
    await tester.scrollUntilVisible(
      find.text('advertising failed'),
      300,
      scrollable: find.byType(Scrollable).last,
    );
    expect(find.text('advertising failed'), findsOneWidget);
  });

  testWidgets('shows an incoming authenticated pairing decision', (
    tester,
  ) async {
    final controller = DiscoveryController(platformOverride: 'macos')
      ..pairingActivity = [
        PairingEvent(
          eventId: BigInt.one,
          requestId: BigInt.from(7),
          kind: PairingEventKind.confirmationRequired,
          peerFingerprint: '12:34:56:78:9A:BC',
          shortCode: '042731',
          alreadyTrusted: false,
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Pairing request'), findsOneWidget);
    expect(find.text('042731'), findsOneWidget);
    expect(find.textContaining('12:34:56:78:9A:BC'), findsOneWidget);
    expect(find.text('Codes match — accept'), findsOneWidget);
    expect(find.text('Reject'), findsOneWidget);
  });

  testWidgets('shows the actionable authenticated connection failure reason', (
    tester,
  ) async {
    const peerId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
    final controller = DiscoveryController(platformOverride: 'android')
      ..peers = [
        DiscoveryPeer(
          presenceId: peerId,
          deviceType: DiscoveryDeviceType.macos,
          compatible: true,
          capabilities: BigInt.zero,
          sources: const ['ble-android'],
          candidateEndpoints: const ['192.0.2.1:4433'],
          candidateCount: 1,
          quarantined: false,
        ),
      ]
      ..pairingActivity = [
        PairingEvent(
          eventId: BigInt.one,
          kind: PairingEventKind.failed,
          peerPresenceId: peerId,
          alreadyTrusted: false,
          detail: 'connect_unreachable',
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Secure pairing failed'), findsOneWidget);
    expect(
      find.text(
        'The discovered address is not reachable on the current network.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('shows that a closed authenticated session must reconnect', (
    tester,
  ) async {
    const peerId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
    final controller = DiscoveryController(platformOverride: 'android')
      ..peers = [
        DiscoveryPeer(
          presenceId: peerId,
          deviceType: DiscoveryDeviceType.macos,
          compatible: true,
          capabilities: BigInt.zero,
          sources: const ['mdns'],
          candidateEndpoints: const ['192.0.2.1:4433'],
          candidateCount: 1,
          quarantined: false,
        ),
      ]
      ..pairingActivity = [
        PairingEvent(
          eventId: BigInt.one,
          kind: PairingEventKind.disconnected,
          peerPresenceId: peerId,
          peerFingerprint: '12:34:56:78:9A:BC',
          alreadyTrusted: true,
          authenticatedSessionId: BigInt.from(7),
          detail: 'transport_closed',
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Secure connection ended'), findsOneWidget);
    expect(
      find.textContaining('Reconnect to start a new authenticated session'),
      findsOneWidget,
    );
  });

  testWidgets('shows an incoming authenticated LAN file offer', (tester) async {
    final controller = DiscoveryController(platformOverride: 'macos')
      ..transferActivity = [
        TransferEvent(
          eventId: BigInt.one,
          requestId: BigInt.from(9),
          authenticatedSessionId: BigInt.from(3),
          transferId: '00112233445566778899aabbccddeeff',
          direction: TransferDirection.receiving,
          kind: TransferEventKind.offerReceived,
          fileName: 'photo.jpg',
          fileNames: const ['photo.jpg'],
          fileSizes: Uint64List.fromList(const [2048]),
          fileSize: BigInt.from(2048),
          transferredBytes: BigInt.zero,
          completedFiles: 0,
          currentFileIndex: 0,
          resumable: true,
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Incoming file'), findsOneWidget);
    expect(find.text('photo.jpg · 2.00 KiB'), findsOneWidget);
    expect(find.text('Accept file'), findsOneWidget);
    expect(find.text('Reject'), findsOneWidget);
    expect(find.textContaining('BLE carries no file bytes'), findsOneWidget);
  });

  testWidgets('shows an incoming resumable multi-file offer', (tester) async {
    final controller = DiscoveryController(platformOverride: 'android')
      ..transferActivity = [
        TransferEvent(
          eventId: BigInt.one,
          requestId: BigInt.from(11),
          authenticatedSessionId: BigInt.from(4),
          transferId: '11223344556677889900aabbccddeeff',
          direction: TransferDirection.receiving,
          kind: TransferEventKind.offerReceived,
          fileName: 'first.txt',
          fileNames: const ['first.txt', 'second.bin'],
          fileSizes: Uint64List.fromList(const [1024, 2048]),
          fileSize: BigInt.from(3072),
          transferredBytes: BigInt.zero,
          completedFiles: 0,
          resumable: true,
        ),
      ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        home: HaloDiscoveryPage(controller: controller),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Incoming files'), findsOneWidget);
    expect(find.text('2 files · 3.00 KiB'), findsOneWidget);
    expect(find.text('first.txt · 1.00 KiB'), findsOneWidget);
    expect(find.text('second.bin · 2.00 KiB'), findsOneWidget);
    expect(find.text('Accept files'), findsOneWidget);
  });
}
