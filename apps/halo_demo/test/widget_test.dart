import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:halo_demo/discovery_controller.dart';
import 'package:halo_demo/l10n/app_localizations.dart';
import 'package:halo_demo/main.dart';
import 'package:halo_demo/src/rust/api.dart';

void main() {
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
}
