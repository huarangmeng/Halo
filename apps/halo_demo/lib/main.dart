import 'package:flutter/material.dart';

import 'discovery_controller.dart';
import 'l10n/app_localizations.dart';
import 'src/rust/api.dart';
import 'src/rust/api/pairing_api.dart';
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

class _HaloAppState extends State<HaloApp> {
  late final DiscoveryController controller;

  @override
  void initState() {
    super.initState();
    controller = DiscoveryController();
    controller.refreshPlatformCapabilities();
  }

  @override
  void dispose() {
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
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            l10n.appTitle,
                            style: const TextStyle(
                              fontSize: 34,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                        ),
                        IconButton.filledTonal(
                          tooltip: l10n.discoveryDiagnostics,
                          onPressed: () =>
                              _showDiagnostics(context, controller),
                          icon: const Icon(Icons.monitor_heart_outlined),
                        ),
                      ],
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
                      child: ListView(
                        children: [
                          if (controller.incomingPairingEvent
                              case final event?) ...[
                            _PairingPanel(
                              event: event,
                              onRespond:
                                  event.requestId == null ||
                                      !controller.canRespondToPairing(
                                        event.requestId!,
                                      )
                                  ? null
                                  : (accepted) => controller.respondToPairing(
                                      event.requestId!,
                                      accepted,
                                    ),
                            ),
                            const SizedBox(height: 16),
                          ],
                          if (controller.incomingTransferOffer
                              case final offer?) ...[
                            _IncomingTransferPanel(
                              event: offer,
                              onRespond: offer.requestId == null
                                  ? null
                                  : (accepted) => controller.respondToTransfer(
                                      offer.requestId!,
                                      accepted,
                                    ),
                            ),
                            const SizedBox(height: 16),
                          ],
                          if (controller.peers.isEmpty)
                            const _EmptyPeers()
                          else
                            for (
                              var index = 0;
                              index < controller.peers.length;
                              index++
                            ) ...[
                              if (index > 0) const SizedBox(height: 10),
                              _PeerCard(
                                peer: controller.peers[index],
                                controller: controller,
                              ),
                            ],
                        ],
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
                Icon(_deviceTypeIcon(deviceType)),
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

Future<void> _showDiagnostics(
  BuildContext context,
  DiscoveryController controller,
) => showModalBottomSheet<void>(
  context: context,
  isScrollControlled: true,
  showDragHandle: true,
  builder: (context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) => _DiagnosticsSheet(controller: controller),
  ),
);

class _DiagnosticsSheet extends StatelessWidget {
  const _DiagnosticsSheet({required this.controller});

  final DiscoveryController controller;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    return SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxHeight: MediaQuery.sizeOf(context).height * 0.82,
        ),
        child: ListView(
          padding: const EdgeInsets.fromLTRB(24, 0, 24, 28),
          children: [
            Text(
              l10n.discoveryDiagnostics,
              style: Theme.of(context).textTheme.headlineSmall,
            ),
            const SizedBox(height: 6),
            Text(l10n.diagnosticsDescription),
            const SizedBox(height: 20),
            _PeerField(
              label: l10n.discoverySessionId,
              value:
                  controller.localPresenceId ?? l10n.discoverySessionIdPending,
            ),
            const SizedBox(height: 8),
            _PeerField(
              label: l10n.diagnosticsSessionState,
              value: _statusLabel(l10n, controller.state),
            ),
            const SizedBox(height: 24),
            Text(
              l10n.diagnosticsCapabilities,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            if (controller.platformCapabilities.isEmpty)
              Text(l10n.diagnosticsNoCapabilities)
            else
              ...controller.platformCapabilities.map(
                (capability) => ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(
                    capability.state == 'ready'
                        ? Icons.check_circle_outline
                        : Icons.warning_amber_outlined,
                    color: capability.state == 'ready'
                        ? Theme.of(context).colorScheme.primary
                        : Theme.of(context).colorScheme.error,
                  ),
                  title: Text(_capabilityNameLabel(l10n, capability.name)),
                  subtitle: Text(
                    '${_providerStateLabel(l10n, capability.state)} · '
                    '${_capabilityDetailLabel(l10n, capability.detail)}',
                  ),
                ),
              ),
            const SizedBox(height: 16),
            Text(
              l10n.diagnosticsProviders,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            if (controller.providerStatuses.isEmpty)
              Text(l10n.diagnosticsNoProviders)
            else
              ...controller.providerStatuses.map(
                (provider) => ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: Icon(
                    provider.state == 'ready'
                        ? Icons.check_circle_outline
                        : Icons.info_outline,
                    color: provider.state == 'ready'
                        ? Theme.of(context).colorScheme.primary
                        : Theme.of(context).colorScheme.error,
                  ),
                  title: Text(provider.name),
                  subtitle: Text(
                    [
                      _providerKindLabel(l10n, provider.kind),
                      _providerStateLabel(l10n, provider.state),
                      if (provider.detail != null) provider.detail!,
                    ].join(' · '),
                  ),
                ),
              ),
            const SizedBox(height: 16),
            Text(
              l10n.diagnosticsRecentEvents,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            if (controller.diagnostics.isEmpty)
              Text(l10n.diagnosticsNoEvents)
            else
              ...controller.diagnostics.map(
                (entry) => ListTile(
                  contentPadding: EdgeInsets.zero,
                  dense: true,
                  leading: const Icon(Icons.article_outlined),
                  title: Text(entry.operation),
                  subtitle: Text(entry.detail),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _PeerCard extends StatelessWidget {
  const _PeerCard({required this.peer, required this.controller});

  final DiscoveryPeer peer;
  final DiscoveryController controller;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final pairing = controller.pairingEventFor(peer);
    final hasConnectablePath = controller.hasConnectablePathFor(peer);
    final pairingInProgress =
        pairing?.kind == PairingEventKind.connecting ||
        pairing?.kind == PairingEventKind.codeAvailable;
    final lanSession = controller.lanSessionForPeer(peer);
    final transfer = lanSession == null
        ? null
        : controller.transferForSession(lanSession.sessionId);
    final transferInProgress =
        transfer?.kind == TransferEventKind.awaitingDecision ||
        transfer?.kind == TransferEventKind.transferring;
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
            if (pairing != null) ...[
              const SizedBox(height: 12),
              _PairingSummary(event: pairing),
            ],
            if (lanSession != null) ...[
              const SizedBox(height: 12),
              Text(
                l10n.transferLanOnly,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
            if (transfer != null) ...[
              const SizedBox(height: 10),
              _TransferSummary(event: transfer),
            ],
            const SizedBox(height: 12),
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: Wrap(
                spacing: 10,
                runSpacing: 8,
                alignment: WrapAlignment.end,
                children: [
                  if (lanSession != null)
                    FilledButton.icon(
                      onPressed:
                          !transferInProgress && controller.platform != 'ios'
                          ? () => controller.sendFileToPeer(peer)
                          : null,
                      icon: const Icon(Icons.upload_file_outlined),
                      label: Text(l10n.transferSendFile),
                    )
                  else
                    FilledButton.icon(
                      onPressed:
                          controller.canPair &&
                              peer.compatible &&
                              hasConnectablePath &&
                              !pairingInProgress
                          ? () => controller.connectToPeer(peer)
                          : null,
                      icon: const Icon(Icons.lock_outline),
                      label: Text(l10n.connectSecurely),
                    ),
                  if (transferInProgress && transfer != null)
                    OutlinedButton(
                      onPressed: () =>
                          controller.cancelTransfer(transfer.transferId),
                      child: Text(l10n.transferCancel),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _IncomingTransferPanel extends StatelessWidget {
  const _IncomingTransferPanel({required this.event, this.onRespond});

  final TransferEvent event;
  final ValueChanged<bool>? onRespond;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    return Card(
      color: Theme.of(context).colorScheme.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              l10n.transferIncomingTitle,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            Text(
              l10n.transferOfferDescription(
                event.fileName,
                _formatBytes(event.fileSize),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              l10n.transferLanOnly,
              style: Theme.of(context).textTheme.bodySmall,
            ),
            if (onRespond != null) ...[
              const SizedBox(height: 14),
              Wrap(
                spacing: 10,
                children: [
                  FilledButton.icon(
                    onPressed: () => onRespond!(true),
                    icon: const Icon(Icons.download_outlined),
                    label: Text(l10n.transferAccept),
                  ),
                  OutlinedButton(
                    onPressed: () => onRespond!(false),
                    child: Text(l10n.transferReject),
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _TransferSummary extends StatelessWidget {
  const _TransferSummary({required this.event});

  final TransferEvent event;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final status = switch (event.kind) {
      TransferEventKind.offerReceived ||
      TransferEventKind.awaitingDecision => l10n.transferAwaitingDecision,
      TransferEventKind.transferring => l10n.transferTransferring,
      TransferEventKind.completed => l10n.transferCompleted,
      TransferEventKind.rejected => l10n.transferRejected,
      TransferEventKind.cancelled => l10n.transferCancelled,
      TransferEventKind.failed => l10n.transferFailed,
    };
    return Semantics(
      liveRegion: true,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('$status · ${event.fileName} · ${_formatBytes(event.fileSize)}'),
          if (event.kind == TransferEventKind.transferring) ...[
            const SizedBox(height: 8),
            LinearProgressIndicator(
              value: event.fileSize == BigInt.zero
                  ? null
                  : (event.transferredBytes.toDouble() /
                            event.fileSize.toDouble())
                        .clamp(0.0, 1.0),
            ),
            const SizedBox(height: 4),
            Text(
              '${_transferPercent(event)} · '
              '${_formatBytes(event.transferredBytes)} / '
              '${_formatBytes(event.fileSize)}',
            ),
          ],
          if (event.finalPath case final path?) ...[
            const SizedBox(height: 4),
            SelectableText(l10n.transferReceivedAt(path)),
          ],
          if (event.detail case final detail?) ...[
            const SizedBox(height: 4),
            Text(
              detail,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ],
        ],
      ),
    );
  }
}

String _formatBytes(BigInt bytes) {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  var value = bytes.toDouble();
  var unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit == 0
      ? '${bytes.toString()} ${units[unit]}'
      : '${value.toStringAsFixed(value >= 10 ? 1 : 2)} ${units[unit]}';
}

String _transferPercent(TransferEvent event) {
  if (event.fileSize == BigInt.zero) return '100%';
  final percent =
      (event.transferredBytes.toDouble() / event.fileSize.toDouble() * 100)
          .clamp(0, 100)
          .toStringAsFixed(0);
  return '$percent%';
}

class _PairingPanel extends StatelessWidget {
  const _PairingPanel({required this.event, this.onRespond});

  final PairingEvent event;
  final ValueChanged<bool>? onRespond;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    return Card(
      color: Theme.of(context).colorScheme.secondaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              l10n.pairingIncomingTitle,
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 10),
            _PairingSummary(event: event),
            if (event.kind == PairingEventKind.confirmationRequired &&
                onRespond != null) ...[
              const SizedBox(height: 14),
              Wrap(
                spacing: 10,
                runSpacing: 8,
                children: [
                  FilledButton.icon(
                    onPressed: () => onRespond!(true),
                    icon: const Icon(Icons.verified_user_outlined),
                    label: Text(l10n.pairingAccept),
                  ),
                  OutlinedButton(
                    onPressed: () => onRespond!(false),
                    child: Text(l10n.pairingReject),
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _PairingSummary extends StatelessWidget {
  const _PairingSummary({required this.event});

  final PairingEvent event;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context);
    final code = event.shortCode;
    final fingerprint = event.peerFingerprint;
    return Semantics(
      liveRegion: true,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(_pairingStatusLabel(l10n, event)),
          if (event.detail case final detail?) ...[
            const SizedBox(height: 6),
            Text(
              _pairingDetailLabel(l10n, detail),
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          ],
          if (code != null) ...[
            const SizedBox(height: 8),
            Text(l10n.pairingCodeLabel),
            SelectableText(
              code,
              style: Theme.of(context).textTheme.headlineMedium?.copyWith(
                fontFamily: 'monospace',
                fontWeight: FontWeight.w700,
                letterSpacing: 5,
              ),
            ),
          ],
          if (fingerprint != null) ...[
            const SizedBox(height: 6),
            _PeerField(label: l10n.pairingFingerprintLabel, value: fingerprint),
          ],
        ],
      ),
    );
  }
}

String _pairingStatusLabel(AppLocalizations l10n, PairingEvent event) =>
    switch (event.kind) {
      PairingEventKind.connecting => l10n.pairingConnecting,
      PairingEventKind.codeAvailable ||
      PairingEventKind.confirmationRequired => l10n.pairingCodeLabel,
      PairingEventKind.trusted =>
        event.alreadyTrusted
            ? l10n.pairingTrustedRecognized
            : l10n.pairingTrusted,
      PairingEventKind.rejected => l10n.pairingRejected,
      PairingEventKind.identityChanged => l10n.pairingIdentityChanged,
      PairingEventKind.timedOut ||
      PairingEventKind.cancelled => l10n.pairingTimedOut,
      PairingEventKind.failed => l10n.pairingFailed,
      PairingEventKind.disconnected => l10n.pairingDisconnected,
    };

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
    switch (platform) {
      'android' => l10n.platformAndroid,
      'ios' => l10n.platformIos,
      _ => l10n.platformMacos,
    };

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

IconData _deviceTypeIcon(DiscoveryDeviceType deviceType) =>
    switch (deviceType) {
      DiscoveryDeviceType.android => Icons.phone_android,
      DiscoveryDeviceType.ios => Icons.phone_iphone,
      DiscoveryDeviceType.macos ||
      DiscoveryDeviceType.windows ||
      DiscoveryDeviceType.linux => Icons.laptop,
      DiscoveryDeviceType.unknown => Icons.devices_other,
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
    DiscoveryNotice.permissionContext =>
      controller.platform == 'android'
          ? l10n.noticePermissionContext
          : l10n.noticeApplePermissionContext,
    DiscoveryNotice.permissionDenied => l10n.noticePermissionDenied,
    DiscoveryNotice.locationServicesDisabled =>
      l10n.noticeLocationServicesDisabled,
    DiscoveryNotice.iosBluetoothPermissionDenied =>
      l10n.noticeIosBluetoothPermissionDenied,
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
    DiscoveryNotice.capabilityHealthDegraded =>
      l10n.noticeCapabilityHealthDegraded(detail),
    DiscoveryNotice.providerHealthDegraded => l10n.noticeProviderHealthDegraded(
      detail,
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
      'failed_recoverable' => l10n.providerFailedRecoverable,
      'failed' => l10n.providerFailed,
      _ => l10n.providerDegraded,
    };

String _providerKindLabel(AppLocalizations l10n, String kind) => switch (kind) {
  'ble' => 'BLE',
  'mdns' => 'mDNS',
  'presence_v4' => l10n.providerPresenceV4,
  'presence_v6' => l10n.providerPresenceV6,
  _ => kind,
};

String _capabilityNameLabel(AppLocalizations l10n, String name) =>
    switch (name) {
      'bluetooth' => l10n.capabilityBluetooth,
      'wifi' => l10n.capabilityWifi,
      'local_network' => l10n.capabilityLocalNetwork,
      'apple_peer_to_peer' => l10n.capabilityApplePeerToPeer,
      'wifi_direct' => l10n.capabilityWifiDirect,
      'wifi_aware' => l10n.capabilityWifiAware,
      'background' => l10n.capabilityBackground,
      _ => name,
    };

String _capabilityDetailLabel(
  AppLocalizations l10n,
  String detail,
) => switch (detail) {
  'bluetooth_ready' || 'ble_ready' => l10n.capabilityBluetoothReady,
  'bluetooth_powered_off' => l10n.capabilityBluetoothOff,
  'bluetooth_permission_missing' || 'bluetooth_permission_not_requested' =>
    l10n.capabilityBluetoothPermissionRequired,
  'bluetooth_permission_denied' => l10n.capabilityBluetoothPermissionDenied,
  'ble_unsupported' => l10n.capabilityBluetoothUnsupported,
  'ble_advertising_unavailable' =>
    l10n.capabilityBluetoothAdvertisingUnavailable,
  'ble_operation_degraded' => l10n.capabilityBluetoothDegraded,
  'bluetooth_resetting' => l10n.capabilityBluetoothResetting,
  'bluetooth_state_checked_when_discovery_starts' =>
    l10n.capabilityBluetoothPending,
  'wifi_connected' => l10n.capabilityWifiConnected,
  'wifi_powered_off' => l10n.capabilityWifiOff,
  'wifi_not_connected' => l10n.capabilityWifiNotConnected,
  'wifi_unsupported' => l10n.capabilityWifiUnsupported,
  'wifi_state_permission_missing' ||
  'wifi_state_unavailable' => l10n.capabilityWifiUnsupported,
  'local_network_connected' => l10n.capabilityLocalNetworkConnected,
  'local_network_socket_bound' => l10n.capabilityLocalNetworkSocketBound,
  'local_network_metered' => l10n.capabilityLocalNetworkMetered,
  'local_network_vpn' => l10n.capabilityLocalNetworkVpn,
  'local_network_binding_failed' => l10n.capabilityLocalNetworkBindingFailed,
  'local_network_not_prepared' => l10n.capabilityLocalNetworkNotPrepared,
  'local_network_restart_required' =>
    l10n.capabilityLocalNetworkRestartRequired,
  'ethernet_connected' => l10n.capabilityEthernetConnected,
  'no_local_network_route' => l10n.capabilityNoLocalNetwork,
  'local_network_permission_missing' =>
    l10n.capabilityLocalNetworkPermissionRequired,
  'apple_p2p_starting' => l10n.capabilityAppleP2PStarting,
  'apple_p2p_ready' => l10n.capabilityAppleP2PReady,
  'apple_p2p_temporarily_unavailable' => l10n.capabilityAppleP2PUnavailable,
  'apple_p2p_failed' => l10n.capabilityAppleP2PFailed,
  'apple_p2p_stopped' => l10n.capabilityAppleP2PStopped,
  'apple_p2p_identity_failed' => l10n.capabilityAppleP2PIdentityFailed,
  'wifi_direct_provider_not_implemented' ||
  'wifi_aware_provider_not_implemented' => l10n.capabilityDirectProviderPending,
  'apple_p2p_unsupported_on_android' ||
  'wifi_direct_unsupported' ||
  'wifi_direct_unsupported_on_apple' ||
  'wifi_aware_unsupported' ||
  'wifi_aware_os_unsupported' ||
  'wifi_aware_unsupported_on_macos' => l10n.capabilityDirectUnsupported,
  'wifi_aware_unavailable' => l10n.capabilityWifiAwareUnavailable,
  'wifi_aware_permission_required' =>
    l10n.capabilityWifiAwarePermissionRequired,
  'network_state_permission_missing' =>
    l10n.capabilityLocalNetworkPermissionRequired,
  'network_state_unavailable' => l10n.capabilityNoLocalNetwork,
  'foreground_service_running' => l10n.capabilityBackgroundRunning,
  'foreground_service_stopped' => l10n.capabilityBackgroundStopped,
  'application_process_background' => l10n.capabilityBackgroundProcess,
  'foreground_only' => l10n.capabilityForegroundOnly,
  _ => detail,
};

String _pairingDetailLabel(AppLocalizations l10n, String detail) =>
    switch (detail) {
      'connect_timeout' => l10n.connectionFailureTimeout,
      'connect_unreachable' => l10n.connectionFailureUnreachable,
      'connect_tls' => l10n.connectionFailureTls,
      'connect_authentication' ||
      'authentication' => l10n.connectionFailureAuthentication,
      'connect_protocol' || 'protocol' => l10n.connectionFailureProtocol,
      'connect_identity_changed' ||
      'identity_changed' => l10n.connectionFailureIdentityChanged,
      'connect_network_changed' => l10n.connectionFailureNetworkChanged,
      'connect_cancelled' || 'cancelled' => l10n.connectionFailureCancelled,
      'connect_configuration' ||
      'configuration' => l10n.connectionFailureConfiguration,
      'control_io' => l10n.connectionFailureControlIo,
      'persistence' => l10n.connectionFailurePersistence,
      'user_interface' => l10n.connectionFailureUserInterface,
      'connect_internal' => l10n.connectionFailureInternal,
      'transport_closed' => l10n.connectionSessionClosed,
      'retry_rate_limited' => l10n.connectionRetryRateLimited,
      _ => l10n.connectionFailureUnknown(detail),
    };
