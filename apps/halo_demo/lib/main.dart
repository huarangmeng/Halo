import 'dart:async';

import 'package:flutter/material.dart';

import 'discovery_controller.dart';
import 'l10n/app_localizations.dart';
import 'src/rust/api.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await HaloRustLib.init();
  runApp(const HaloApp());
}

class HaloApp extends StatefulWidget {
  const HaloApp({super.key});

  @override
  State<HaloApp> createState() => _HaloAppState();
}

class _HaloAppState extends State<HaloApp> with WidgetsBindingObserver {
  late final DiscoveryController controller;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    controller = DiscoveryController();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if ((state == AppLifecycleState.paused ||
            state == AppLifecycleState.hidden ||
            state == AppLifecycleState.detached) &&
        controller.hasActiveWork) {
      unawaited(controller.stop());
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    onGenerateTitle: (context) => AppLocalizations.of(context).appTitle,
    localizationsDelegates: AppLocalizations.localizationsDelegates,
    supportedLocales: AppLocalizations.supportedLocales,
    debugShowCheckedModeBanner: false,
    theme: ThemeData(
      colorScheme: ColorScheme.fromSeed(
        seedColor: const Color(0xff5b66e8),
        brightness: Brightness.light,
      ),
      scaffoldBackgroundColor: const Color(0xfff7f8fc),
      useMaterial3: true,
    ),
    home: HaloDiscoveryPage(controller: controller),
  );
}

class HaloDiscoveryPage extends StatelessWidget {
  const HaloDiscoveryPage({required this.controller, super.key});

  final DiscoveryController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) {
      final l10n = AppLocalizations.of(context);
      return Scaffold(
        body: SafeArea(
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 820),
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text(
                      l10n.appTitle,
                      style: const TextStyle(
                        fontSize: 34,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 6),
                    Text(
                      l10n.appSubtitle(
                        _platformLabel(l10n, controller.platform),
                      ),
                      style: Theme.of(context).textTheme.bodyLarge,
                    ),
                    const SizedBox(height: 16),
                    _LocalDevicePanel(controller: controller),
                    const SizedBox(height: 16),
                    _StatusPanel(controller: controller),
                    const SizedBox(height: 16),
                    Row(
                      children: [
                        Expanded(
                          child: FilledButton.icon(
                            onPressed: controller.canStart
                                ? controller.start
                                : null,
                            icon: const Icon(Icons.radar),
                            label: Text(l10n.startDiscovery),
                          ),
                        ),
                        const SizedBox(width: 12),
                        OutlinedButton.icon(
                          onPressed: controller.canStop
                              ? controller.stop
                              : null,
                          icon: const Icon(Icons.stop_circle_outlined),
                          label: Text(l10n.stop),
                        ),
                      ],
                    ),
                    const SizedBox(height: 28),
                    Text(
                      l10n.nearbyDevices(controller.peers.length),
                      style: Theme.of(context).textTheme.titleLarge,
                    ),
                    const SizedBox(height: 12),
                    Expanded(
                      child: controller.peers.isEmpty
                          ? const _EmptyPeers()
                          : ListView.separated(
                              itemCount: controller.peers.length,
                              separatorBuilder: (_, _) =>
                                  const SizedBox(height: 10),
                              itemBuilder: (context, index) {
                                final peer = controller.peers[index];
                                return _PeerCard(peer: peer);
                              },
                            ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      );
    },
  );
}

class _LocalDevicePanel extends StatelessWidget {
  const _LocalDevicePanel({required this.controller});

  final DiscoveryController controller;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final deviceType =
        controller.localDeviceType ?? controller.platformDeviceType;
    final presenceId = controller.localPresenceId;
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                const Icon(Icons.laptop_mac),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    l10n.thisDevice,
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                Chip(label: Text(_deviceTypeLabel(l10n, deviceType))),
              ],
            ),
            const SizedBox(height: 10),
            Text(
              l10n.discoverySessionId,
              style: Theme.of(context).textTheme.labelMedium,
            ),
            const SizedBox(height: 3),
            SelectableText(
              presenceId ?? l10n.discoverySessionIdPending,
              key: const ValueKey('local-presence-id'),
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(fontFamily: 'monospace'),
            ),
          ],
        ),
      ),
    );
  }
}

class _PeerCard extends StatelessWidget {
  const _PeerCard({required this.peer});

  final DiscoveryPeer peer;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                CircleAvatar(
                  child: Icon(
                    peer.compatible ? Icons.devices : Icons.device_unknown,
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    l10n.nearbyHaloDevice,
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                Chip(
                  label: Text(
                    peer.compatible ? l10n.compatible : l10n.incompatible,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 12),
            _PeerField(
              label: l10n.deviceTypeLabel,
              value: _deviceTypeLabel(l10n, peer.deviceType),
            ),
            const SizedBox(height: 8),
            Text(
              l10n.peerIdLabel,
              style: Theme.of(context).textTheme.labelMedium,
            ),
            const SizedBox(height: 3),
            SelectableText(
              peer.presenceId,
              key: ValueKey('peer-presence-id-${peer.presenceId}'),
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(fontFamily: 'monospace'),
            ),
            const SizedBox(height: 8),
            _PeerField(
              label: l10n.discoverySourcesLabel,
              value: peer.sources.join(' + '),
            ),
            const SizedBox(height: 8),
            _PeerField(
              label: l10n.endpointLabel,
              value: peer.bestEndpoint ?? l10n.bleAwaitingLan,
            ),
          ],
        ),
      ),
    );
  }
}

class _PeerField extends StatelessWidget {
  const _PeerField({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) => Text.rich(
    TextSpan(
      children: [
        TextSpan(
          text: '$label: ',
          style: Theme.of(context).textTheme.labelMedium,
        ),
        TextSpan(text: value),
      ],
    ),
  );
}

class _StatusPanel extends StatelessWidget {
  const _StatusPanel({required this.controller});

  final DiscoveryController controller;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final status = _statusLabel(l10n, controller.state);
    return Semantics(
      liveRegion: true,
      label: l10n.discoveryStatusSemantics(status),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.surface,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(
            color: Theme.of(context).colorScheme.outlineVariant,
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.all(18),
          child: Row(
            children: [
              Icon(
                controller.isRunning
                    ? Icons.bluetooth_searching
                    : Icons.bluetooth_disabled,
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      status,
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 4),
                    Text(
                      _noticeText(l10n, controller),
                      style: Theme.of(context).textTheme.bodyMedium,
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _EmptyPeers extends StatelessWidget {
  const _EmptyPeers();

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Text(
          l10n.emptyPeers,
          textAlign: TextAlign.center,
          style: Theme.of(context).textTheme.bodyLarge,
        ),
      ),
    );
  }
}

String _platformLabel(AppLocalizations l10n, String platform) =>
    platform == 'android' ? l10n.platformAndroid : l10n.platformMacos;

String _deviceTypeLabel(
  AppLocalizations l10n,
  DiscoveryDeviceType deviceType,
) => switch (deviceType) {
  DiscoveryDeviceType.android => l10n.platformAndroid,
  DiscoveryDeviceType.ios => l10n.platformIos,
  DiscoveryDeviceType.macos => l10n.platformMacos,
  DiscoveryDeviceType.windows => l10n.platformWindows,
  DiscoveryDeviceType.linux => l10n.platformLinux,
  DiscoveryDeviceType.unknown => l10n.deviceTypeUnknown,
};

String _statusLabel(AppLocalizations l10n, DiscoveryRunState state) =>
    switch (state) {
      DiscoveryRunState.stopped => l10n.statusStopped,
      DiscoveryRunState.preparing => l10n.statusPreparing,
      DiscoveryRunState.starting => l10n.statusStarting,
      DiscoveryRunState.running => l10n.statusRunning,
      DiscoveryRunState.degraded => l10n.statusDegraded,
      DiscoveryRunState.failed => l10n.statusFailed,
    };

String _noticeText(AppLocalizations l10n, DiscoveryController controller) {
  final detail = controller.noticeDetail ?? '';
  return switch (controller.notice) {
    DiscoveryNotice.stopped => l10n.noticeStopped,
    DiscoveryNotice.permissionContext => l10n.noticePermissionContext,
    DiscoveryNotice.permissionDenied => l10n.noticePermissionDenied,
    DiscoveryNotice.locationServicesDisabled =>
      l10n.noticeLocationServicesDisabled,
    DiscoveryNotice.macosBluetoothPermissionDenied =>
      l10n.noticeMacosBluetoothPermissionDenied,
    DiscoveryNotice.starting => l10n.noticeStarting,
    DiscoveryNotice.running => l10n.noticeRunning,
    DiscoveryNotice.nativeEventStopped => l10n.noticeNativeEventStopped(detail),
    DiscoveryNotice.startFailed => l10n.noticeStartFailed(detail),
    DiscoveryNotice.cleanupFailed => l10n.noticeCleanupFailed(detail),
    DiscoveryNotice.bleUnavailable => l10n.noticeBleUnavailable(
      _providerStateLabel(l10n, detail),
    ),
    DiscoveryNotice.diagnostic => l10n.noticeDiagnostic(
      controller.noticeOperation ?? 'BLE',
      detail,
    ),
    DiscoveryNotice.rustRejected => l10n.noticeRustRejected(detail),
  };
}

String _providerStateLabel(AppLocalizations l10n, String state) =>
    switch (state) {
      'starting' => l10n.providerStarting,
      'ready' => l10n.providerReady,
      'permission_required' => l10n.providerPermissionRequired,
      'permission_denied' => l10n.providerPermissionDenied,
      'hardware_off' => l10n.providerHardwareOff,
      'unsupported' => l10n.providerUnsupported,
      'temporarily_unavailable' => l10n.providerTemporarilyUnavailable,
      'stopped' => l10n.providerStopped,
      _ => l10n.providerDegraded,
    };
