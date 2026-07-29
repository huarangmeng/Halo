import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'src/rust/api.dart';
import 'src/rust/api/pairing_api.dart';

enum DiscoveryRunState {
  stopped,
  preparing,
  starting,
  running,
  degraded,
  failed,
}

enum DiscoveryNotice {
  stopped,
  permissionContext,
  permissionDenied,
  locationServicesDisabled,
  iosBluetoothPermissionDenied,
  macosBluetoothPermissionDenied,
  starting,
  running,
  nativeEventStopped,
  startFailed,
  cleanupFailed,
  bleUnavailable,
  capabilityHealthDegraded,
  providerHealthDegraded,
  diagnostic,
  rustRejected,
}

class DiscoveryDiagnosticEntry {
  const DiscoveryDiagnosticEntry({
    required this.operation,
    required this.detail,
  });

  final String operation;
  final String detail;
}

class PlatformCapabilityStatus {
  const PlatformCapabilityStatus({
    required this.name,
    required this.state,
    required this.detail,
  });

  final String name;
  final String state;
  final String detail;
}

class DiscoveryController extends ChangeNotifier {
  static const _methods = MethodChannel('org.halo.discovery/ble');
  static const _events = EventChannel('org.halo.discovery/ble-events');
  static const _identityMethods = MethodChannel('org.halo.identity/storage');

  DiscoveryController({String? platformOverride})
    : platform = platformOverride ?? _detectedPlatform();

  DiscoveryRunState state = DiscoveryRunState.stopped;
  DiscoveryNotice notice = DiscoveryNotice.stopped;
  String? noticeDetail;
  String? noticeOperation;
  String? localPresenceId;
  DiscoveryDeviceType? localDeviceType;
  List<DiscoveryPeer> peers = const [];
  List<PlatformCapabilityStatus> platformCapabilities = const [];
  List<DiscoveryProviderStatus> providerStatuses = const [];
  List<DiscoveryDiagnosticEntry> diagnostics = const [];
  List<PairingEvent> pairingActivity = const [];

  final String platform;

  BigInt? _sessionId;
  BigInt? _pairingSessionId;
  BigInt _lastPairingEventId = BigInt.zero;
  StreamSubscription<Object?>? _nativeEvents;
  Timer? _snapshotTimer;
  Future<void>? _startOperation;
  Future<void>? _stopOperation;
  bool _stopRequested = false;
  bool _refreshing = false;
  final Set<BigInt> _respondedPairingRequests = <BigInt>{};

  bool get canStart =>
      _startOperation == null &&
      _stopOperation == null &&
      (state == DiscoveryRunState.stopped || state == DiscoveryRunState.failed);
  bool get canStop => _sessionId != null;
  bool get canPair => _pairingSessionId != null;
  bool get hasActiveWork =>
      state != DiscoveryRunState.stopped || _startOperation != null;
  bool get isRunning =>
      state == DiscoveryRunState.running || state == DiscoveryRunState.degraded;
  DiscoveryDeviceType get platformDeviceType => switch (platform) {
    'android' => DiscoveryDeviceType.android,
    'ios' => DiscoveryDeviceType.ios,
    _ => DiscoveryDeviceType.macos,
  };

  Future<void> start() {
    if (!canStart) return Future<void>.value();
    _stopRequested = false;
    final operation = _start();
    _startOperation = operation;
    return operation.whenComplete(() {
      if (identical(_startOperation, operation)) _startOperation = null;
    });
  }

  Future<void> refreshPlatformCapabilities({bool notify = true}) async {
    try {
      final raw = await _methods.invokeMethod<Object?>('capabilities');
      if (raw is! List<Object?>) return;
      platformCapabilities = raw
          .whereType<Map<Object?, Object?>>()
          .map(
            (entry) => PlatformCapabilityStatus(
              name: entry['name'] as String? ?? 'unknown',
              state: entry['state'] as String? ?? 'degraded',
              detail: entry['detail'] as String? ?? 'unknown',
            ),
          )
          .toList(growable: false);
      if (notify) notifyListeners();
    } on MissingPluginException {
      // Widget tests and unsupported launchers have no platform capability bridge.
    } catch (_) {
      // Capability polling must never stop healthy discovery providers.
    }
  }

  Future<void> _start() async {
    peers = const [];
    providerStatuses = const [];
    diagnostics = const [];
    pairingActivity = const [];
    _lastPairingEventId = BigInt.zero;
    _respondedPairingRequests.clear();
    state = DiscoveryRunState.preparing;
    _setNotice(DiscoveryNotice.permissionContext);
    notifyListeners();

    try {
      final preparation = await _methods.invokeMethod<Object?>('prepare');
      if (preparation is Map<Object?, Object?> &&
          preparation['capabilities'] is List<Object?>) {
        final rawCapabilities = preparation['capabilities']! as List<Object?>;
        platformCapabilities = rawCapabilities
            .whereType<Map<Object?, Object?>>()
            .map(
              (entry) => PlatformCapabilityStatus(
                name: entry['name'] as String? ?? 'unknown',
                state: entry['state'] as String? ?? 'degraded',
                detail: entry['detail'] as String? ?? 'unknown',
              ),
            )
            .toList(growable: false);
      }
      final prepared = switch (preparation) {
        final bool value => value,
        final Map<Object?, Object?> value => value['ready'] == true,
        _ => false,
      };
      if (!prepared) {
        state = DiscoveryRunState.failed;
        final reason = preparation is Map<Object?, Object?>
            ? preparation['reason']
            : null;
        _setNotice(switch (reason) {
          'location_services_disabled' =>
            DiscoveryNotice.locationServicesDisabled,
          'permission_denied' when platform == 'macos' =>
            DiscoveryNotice.macosBluetoothPermissionDenied,
          'permission_denied' when platform == 'ios' =>
            DiscoveryNotice.iosBluetoothPermissionDenied,
          _ => DiscoveryNotice.permissionDenied,
        });
        notifyListeners();
        return;
      }
      if (_stopRequested) {
        state = DiscoveryRunState.stopped;
        _setNotice(DiscoveryNotice.stopped);
        notifyListeners();
        return;
      }

      state = DiscoveryRunState.starting;
      _setNotice(DiscoveryNotice.starting);
      notifyListeners();

      _nativeEvents = _events.receiveBroadcastStream().listen(
        _handleNativeEvent,
        onError: (Object error, StackTrace stackTrace) {
          state = DiscoveryRunState.degraded;
          _setNotice(
            DiscoveryNotice.nativeEventStopped,
            detail: _safeError(error),
          );
          notifyListeners();
        },
      );
      final identityBlob = await _identityMethods.invokeMethod<Uint8List>(
        'load',
      );
      final trustStoreDirectory = await _identityMethods.invokeMethod<String>(
        'trustStoreDirectory',
      );
      if (trustStoreDirectory == null || trustStoreDirectory.isEmpty) {
        throw StateError('Platform trust storage directory is unavailable');
      }
      final pairingBootstrap = await pairingStart(
        identityBlob: identityBlob,
        trustStoreDirectory: trustStoreDirectory,
      );
      _pairingSessionId = pairingBootstrap.sessionId;
      final newIdentityBlob = pairingBootstrap.identityBlobToPersist;
      if (newIdentityBlob != null) {
        await _identityMethods.invokeMethod<void>('save', <String, Object>{
          'blob': newIdentityBlob,
        });
      }
      final bootstrap = await discoveryStart(
        quicPort: pairingBootstrap.listenPort,
        enableLan: true,
        deviceType: platformDeviceType,
      );
      _sessionId = bootstrap.sessionId;
      localPresenceId = bootstrap.presenceId;
      localDeviceType = bootstrap.deviceType;
      if (_stopRequested) {
        await _cleanupAfterFailedStart();
        state = DiscoveryRunState.stopped;
        _setNotice(DiscoveryNotice.stopped);
        notifyListeners();
        return;
      }
      await _methods.invokeMethod<void>('start', <String, Object>{
        'presence': bootstrap.blePresence,
      });
      if (_stopRequested) {
        await _cleanupAfterFailedStart();
        state = DiscoveryRunState.stopped;
        _setNotice(DiscoveryNotice.stopped);
        notifyListeners();
        return;
      }
      _snapshotTimer = Timer.periodic(
        const Duration(seconds: 1),
        (_) => unawaited(_refreshSnapshot()),
      );
      state = DiscoveryRunState.running;
      _setNotice(DiscoveryNotice.running);
      await _refreshSnapshot();
      notifyListeners();
    } catch (error) {
      await _cleanupAfterFailedStart();
      state = DiscoveryRunState.failed;
      _setNotice(DiscoveryNotice.startFailed, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> stop() {
    _stopRequested = true;
    final existing = _stopOperation;
    if (existing != null) return existing;
    final operation = _stopAfterStart();
    _stopOperation = operation;
    return operation.whenComplete(() {
      if (identical(_stopOperation, operation)) _stopOperation = null;
    });
  }

  Future<void> _stopAfterStart() async {
    await _startOperation;
    final sessionId = _sessionId;
    final pairingSessionId = _pairingSessionId;
    _sessionId = null;
    _pairingSessionId = null;
    _snapshotTimer?.cancel();
    _snapshotTimer = null;
    await _nativeEvents?.cancel();
    _nativeEvents = null;
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
      if (pairingSessionId != null) {
        await pairingStop(sessionId: pairingSessionId);
      }
    } catch (error) {
      state = DiscoveryRunState.failed;
      _setNotice(DiscoveryNotice.cleanupFailed, detail: _safeError(error));
      notifyListeners();
      return;
    }
    peers = const [];
    providerStatuses = const [];
    diagnostics = const [];
    pairingActivity = const [];
    _respondedPairingRequests.clear();
    localPresenceId = null;
    localDeviceType = null;
    state = DiscoveryRunState.stopped;
    _setNotice(DiscoveryNotice.stopped);
    notifyListeners();
  }

  Future<void> _handleNativeEvent(Object? rawEvent) async {
    final sessionId = _sessionId;
    if (sessionId == null || rawEvent is! Map<Object?, Object?>) return;
    final type = rawEvent['type'];
    try {
      if (type == 'presence') {
        final descriptor = rawEvent['descriptor'];
        if (descriptor is! Uint8List) return;
        peers = await discoverySubmitBle(
          sessionId: sessionId,
          platform: platform,
          descriptor: descriptor,
        );
        notifyListeners();
      } else if (type == 'state') {
        final rawState = rawEvent['state'] as String? ?? 'degraded';
        final detail = rawEvent['detail'] as String?;
        await discoveryReportBleState(
          sessionId: sessionId,
          platform: platform,
          state: _providerState(rawState),
          detail: detail,
        );
        _upsertCapability(
          PlatformCapabilityStatus(
            name: 'bluetooth',
            state: rawState,
            detail: detail ?? 'bluetooth_$rawState',
          ),
        );
        if (rawState == 'ready') {
          state = DiscoveryRunState.running;
          _setNotice(DiscoveryNotice.running);
        } else if (rawState != 'starting') {
          state = DiscoveryRunState.degraded;
          _setNotice(
            rawState == 'permission_denied' && platform == 'macos'
                ? DiscoveryNotice.macosBluetoothPermissionDenied
                : rawState == 'permission_denied' && platform == 'ios'
                ? DiscoveryNotice.iosBluetoothPermissionDenied
                : DiscoveryNotice.bleUnavailable,
            detail: rawState,
          );
        }
        notifyListeners();
      } else if (type == 'diagnostic') {
        final operation = '${rawEvent['operation']}';
        final detail = '${rawEvent['detail']}';
        diagnostics = [
          DiscoveryDiagnosticEntry(operation: operation, detail: detail),
          ...diagnostics.take(11),
        ];
        _setNotice(
          DiscoveryNotice.diagnostic,
          operation: operation,
          detail: detail,
        );
        notifyListeners();
      }
    } catch (error) {
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> _refreshSnapshot() async {
    final sessionId = _sessionId;
    if (sessionId == null || _refreshing) return;
    _refreshing = true;
    try {
      await refreshPlatformCapabilities(notify: false);
      peers = await discoverySnapshot(sessionId: sessionId);
      providerStatuses = await discoveryProviderStatuses(sessionId: sessionId);
      final pairingSessionId = _pairingSessionId;
      if (pairingSessionId != null) {
        final events = await pairingEvents(
          sessionId: pairingSessionId,
          afterEventId: _lastPairingEventId,
        );
        if (events.isNotEmpty) {
          _lastPairingEventId = events.last.eventId;
          pairingActivity = [...pairingActivity, ...events].reversed
              .take(32)
              .toList(growable: false)
              .reversed
              .toList(growable: false);
        }
      }
      _applyProviderHealth();
      notifyListeners();
    } catch (_) {
      // A simultaneous stop closes the session; stop owns the visible state.
    } finally {
      _refreshing = false;
    }
  }

  Future<void> _cleanupAfterFailedStart() async {
    _snapshotTimer?.cancel();
    _snapshotTimer = null;
    await _nativeEvents?.cancel();
    _nativeEvents = null;
    final sessionId = _sessionId;
    final pairingSessionId = _pairingSessionId;
    _sessionId = null;
    _pairingSessionId = null;
    localPresenceId = null;
    localDeviceType = null;
    providerStatuses = const [];
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
      if (pairingSessionId != null) {
        await pairingStop(sessionId: pairingSessionId);
      }
    } catch (_) {
      // Preserve the original start failure.
    }
  }

  PairingEvent? pairingEventFor(DiscoveryPeer peer) {
    for (final event in pairingActivity.reversed) {
      if (event.peerPresenceId == peer.presenceId) return event;
    }
    return null;
  }

  PairingEvent? get incomingPairingEvent {
    for (final event in pairingActivity.reversed) {
      if (event.peerPresenceId == null &&
          (event.kind == PairingEventKind.confirmationRequired ||
              event.kind == PairingEventKind.trusted ||
              event.kind == PairingEventKind.failed ||
              event.kind == PairingEventKind.identityChanged)) {
        return event;
      }
    }
    return null;
  }

  Future<void> connectToPeer(DiscoveryPeer peer) async {
    final sessionId = _pairingSessionId;
    if (sessionId == null || !peer.compatible) return;
    final endpoints = <String>{
      ...peer.candidateEndpoints,
      ?peer.bestEndpoint,
    }.toList(growable: false);
    if (endpoints.isEmpty) return;
    try {
      await pairingConnect(
        sessionId: sessionId,
        peerPresenceId: peer.presenceId,
        endpoints: endpoints,
      );
      await _refreshSnapshot();
    } catch (error) {
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> respondToPairing(BigInt requestId, bool accepted) async {
    final sessionId = _pairingSessionId;
    if (sessionId == null || !_respondedPairingRequests.add(requestId)) return;
    notifyListeners();
    try {
      await pairingRespond(
        sessionId: sessionId,
        requestId: requestId,
        accepted: accepted,
      );
      await _refreshSnapshot();
    } catch (error) {
      _respondedPairingRequests.remove(requestId);
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  bool canRespondToPairing(BigInt requestId) =>
      !_respondedPairingRequests.contains(requestId);

  PlatformProviderState _providerState(String value) => switch (value) {
    'starting' => PlatformProviderState.starting,
    'ready' => PlatformProviderState.ready,
    'permission_required' => PlatformProviderState.permissionRequired,
    'permission_denied' => PlatformProviderState.permissionDenied,
    'hardware_off' => PlatformProviderState.hardwareOff,
    'unsupported' => PlatformProviderState.unsupported,
    'temporarily_unavailable' => PlatformProviderState.temporarilyUnavailable,
    'stopped' => PlatformProviderState.stopped,
    _ => PlatformProviderState.degraded,
  };

  void _applyProviderHealth() {
    if (!isRunning) return;
    final unhealthyProviders = providerStatuses
        .where(
          (provider) =>
              provider.state != 'ready' && provider.state != 'starting',
        )
        .map((provider) => '${provider.name}:${provider.state}')
        .toList(growable: false);
    final unhealthyCapabilities = platformCapabilities
        .where(
          (capability) =>
              (capability.name == 'bluetooth' ||
                  capability.name == 'local_network') &&
              capability.state != 'ready' &&
              capability.state != 'starting',
        )
        .map((capability) => '${capability.name}:${capability.detail}')
        .toList(growable: false);
    if (unhealthyProviders.isNotEmpty) {
      state = DiscoveryRunState.degraded;
      if (notice == DiscoveryNotice.running ||
          notice == DiscoveryNotice.starting) {
        _setNotice(
          DiscoveryNotice.providerHealthDegraded,
          detail: unhealthyProviders.join(', '),
        );
      }
    } else if (unhealthyCapabilities.isNotEmpty) {
      state = DiscoveryRunState.degraded;
      if (notice == DiscoveryNotice.running ||
          notice == DiscoveryNotice.starting) {
        _setNotice(
          DiscoveryNotice.capabilityHealthDegraded,
          detail: unhealthyCapabilities.join(', '),
        );
      }
    } else if (providerStatuses.any((provider) => provider.state == 'ready')) {
      state = DiscoveryRunState.running;
      _setNotice(DiscoveryNotice.running);
    }
  }

  void _upsertCapability(PlatformCapabilityStatus capability) {
    platformCapabilities = [
      capability,
      ...platformCapabilities.where((entry) => entry.name != capability.name),
    ];
  }

  String _safeError(Object error) {
    final text = error.toString();
    return text.length <= 180 ? text : '${text.substring(0, 180)}…';
  }

  void _setNotice(DiscoveryNotice value, {String? detail, String? operation}) {
    notice = value;
    noticeDetail = detail;
    noticeOperation = operation;
  }

  @override
  void dispose() {
    _stopRequested = true;
    final sessionId = _sessionId;
    final pairingSessionId = _pairingSessionId;
    _sessionId = null;
    _pairingSessionId = null;
    _snapshotTimer?.cancel();
    _nativeEvents?.cancel();
    if (sessionId != null || pairingSessionId != null) {
      unawaited(_disposeNative(sessionId, pairingSessionId));
    }
    super.dispose();
  }

  Future<void> _disposeNative(
    BigInt? sessionId,
    BigInt? pairingSessionId,
  ) async {
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
      if (pairingSessionId != null) {
        await pairingStop(sessionId: pairingSessionId);
      }
    } catch (_) {
      // Application teardown cannot surface an actionable error.
    }
  }
}

String _detectedPlatform() => Platform.isAndroid
    ? 'android'
    : Platform.isIOS
    ? 'ios'
    : 'macos';
