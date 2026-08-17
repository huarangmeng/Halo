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
  List<AuthenticatedSessionInfo> authenticatedLanSessions = const [];
  List<TransferEvent> transferActivity = const [];

  final String platform;

  BigInt? _sessionId;
  BigInt? _pairingSessionId;
  BigInt _lastPairingEventId = BigInt.zero;
  BigInt _lastTransferEventId = BigInt.zero;
  StreamSubscription<Object?>? _nativeEvents;
  Timer? _snapshotTimer;
  Future<void>? _startOperation;
  Future<void>? _stopOperation;
  Future<void>? _hotspotOperation;
  bool _stopRequested = false;
  bool _refreshing = false;
  final Set<BigInt> _respondedPairingRequests = <BigInt>{};
  final Set<BigInt> _respondedTransferRequests = <BigInt>{};
  final Map<String, String> _outgoingSourcesByTransfer = <String, String>{};
  final Map<String, String> _appleP2PCandidatesByPresence = <String, String>{};
  final Set<String> _authenticatedAppleP2PSessions = <String>{};

  bool get canStart =>
      _startOperation == null &&
      _stopOperation == null &&
      _hotspotOperation == null &&
      (state == DiscoveryRunState.stopped || state == DiscoveryRunState.failed);
  bool get canStop => _sessionId != null;
  bool get canJoinLocalHotspot =>
      (platform == 'android' || platform == 'macos') &&
      state == DiscoveryRunState.stopped &&
      _hotspotOperation == null &&
      !hasJoinedLocalHotspot;
  bool get canLeaveLocalHotspot =>
      (platform == 'android' || platform == 'macos') &&
      state == DiscoveryRunState.stopped &&
      _hotspotOperation == null &&
      hasJoinedLocalHotspot;
  bool get hasJoinedLocalHotspot => platformCapabilities.any(
    (capability) =>
        capability.name == 'local_hotspot' && capability.state == 'ready',
  );
  bool get canPair => _pairingSessionId != null;
  bool get hasEligibleLocalNetworkPath => platformCapabilities.any(
    (capability) =>
        capability.name == 'local_network' && capability.state == 'ready',
  );
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

  Future<void> joinLocalHotspot() => _runHotspotOperation('joinLocalHotspot');

  Future<void> leaveLocalHotspot() => _runHotspotOperation('leaveLocalHotspot');

  Future<void> _runHotspotOperation(String method) {
    if (_hotspotOperation != null ||
        (platform != 'android' && platform != 'macos')) {
      return Future<void>.value();
    }
    final operation = () async {
      try {
        final response = await _methods.invokeMethod<Object?>(method);
        final detail = response is Map<Object?, Object?>
            ? response['detail'] as String?
            : null;
        if (detail != null) {
          _setNotice(
            DiscoveryNotice.diagnostic,
            operation: 'local_hotspot',
            detail: detail,
          );
        }
        await refreshPlatformCapabilities(notify: false);
      } on MissingPluginException {
        // Unsupported launchers and widget tests do not expose hotspot setup.
      } catch (error) {
        _setNotice(
          DiscoveryNotice.diagnostic,
          operation: 'local_hotspot',
          detail: _safeError(error),
        );
      } finally {
        notifyListeners();
      }
    }();
    _hotspotOperation = operation;
    notifyListeners();
    return operation.whenComplete(() {
      if (identical(_hotspotOperation, operation)) _hotspotOperation = null;
      notifyListeners();
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
    transferActivity = const [];
    _lastPairingEventId = BigInt.zero;
    _lastTransferEventId = BigInt.zero;
    _respondedPairingRequests.clear();
    _respondedTransferRequests.clear();
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
      final platformTlsIdentity = platform == 'ios' || platform == 'macos'
          ? await pairingCreatePlatformTlsIdentity()
          : null;
      await _methods.invokeMethod<void>('start', <String, Object>{
        'presence': bootstrap.blePresence,
        if (platformTlsIdentity != null) ...<String, Object>{
          'p2pInstanceName': bootstrap.presenceId,
          'pairingSessionId': pairingBootstrap.sessionId.toInt(),
          'platformTlsCertificateDer': platformTlsIdentity.certificateDer,
          'platformTlsPrivateKeyX963': platformTlsIdentity.privateKeyX963,
        },
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
    final outgoingSources = _outgoingSourcesByTransfer.values.toList(
      growable: false,
    );
    _outgoingSourcesByTransfer.clear();
    _sessionId = null;
    _pairingSessionId = null;
    _snapshotTimer?.cancel();
    _snapshotTimer = null;
    await _nativeEvents?.cancel();
    _nativeEvents = null;
    _appleP2PCandidatesByPresence.clear();
    _authenticatedAppleP2PSessions.clear();
    authenticatedLanSessions = const [];
    transferActivity = const [];
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
      if (pairingSessionId != null) {
        await pairingStop(sessionId: pairingSessionId);
      }
    } catch (error) {
      await _discardOutgoingSources(outgoingSources);
      state = DiscoveryRunState.failed;
      _setNotice(DiscoveryNotice.cleanupFailed, detail: _safeError(error));
      notifyListeners();
      return;
    }
    await _discardOutgoingSources(outgoingSources);
    peers = const [];
    providerStatuses = const [];
    diagnostics = const [];
    pairingActivity = const [];
    _respondedPairingRequests.clear();
    _respondedTransferRequests.clear();
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
      } else if (type == 'data_channel_state') {
        final kind = rawEvent['kind'] as String?;
        if (kind != 'apple_peer_to_peer') return;
        final rawState = rawEvent['state'] as String? ?? 'degraded';
        final detail = rawEvent['detail'] as String? ?? 'apple_p2p_$rawState';
        _upsertCapability(
          PlatformCapabilityStatus(
            name: 'apple_peer_to_peer',
            state: rawState,
            detail: detail,
          ),
        );
        notifyListeners();
      } else if (type == 'data_channel_candidate') {
        final peerPresenceId = rawEvent['peerPresenceId'] as String?;
        final handle = rawEvent['handle'] as String?;
        if (peerPresenceId == null || handle == null) return;
        final key = peerPresenceId.toLowerCase();
        if (rawEvent['action'] == 'found') {
          _appleP2PCandidatesByPresence[key] = handle;
        } else if (_appleP2PCandidatesByPresence[key] == handle) {
          _appleP2PCandidatesByPresence.remove(key);
        }
      } else if (type == 'data_channel_link_ready') {
        // Swift connects the native QUIC stream directly to Rust. Dart only
        // observes lifecycle state and never receives exporter or wire bytes.
      } else if (type == 'data_channel_session_ready') {
        final handle = rawEvent['handle'] as String?;
        if (handle != null) _authenticatedAppleP2PSessions.add(handle);
      } else if (type == 'data_channel_link_failed') {
        final handle = rawEvent['handle'] as String?;
        if (handle != null) _authenticatedAppleP2PSessions.remove(handle);
        diagnostics = [
          DiscoveryDiagnosticEntry(
            operation: 'apple_peer_to_peer',
            detail: '${rawEvent['detail'] ?? 'link_failed'}',
          ),
          ...diagnostics.take(11),
        ];
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
        authenticatedLanSessions = await pairingAuthenticatedSessions(
          sessionId: pairingSessionId,
        );
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
        final transferEvents = await pairingTransferEvents(
          sessionId: pairingSessionId,
          afterEventId: _lastTransferEventId,
        );
        if (transferEvents.isNotEmpty) {
          _lastTransferEventId = transferEvents.last.eventId;
          transferActivity = [...transferActivity, ...transferEvents].reversed
              .take(64)
              .toList(growable: false)
              .reversed
              .toList(growable: false);
          await _discardFinishedOutgoingSources(transferEvents);
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
    _appleP2PCandidatesByPresence.clear();
    _authenticatedAppleP2PSessions.clear();
    authenticatedLanSessions = const [];
    transferActivity = const [];
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
    try {
      final appleCandidate =
          _appleP2PCandidatesByPresence[peer.presenceId.toLowerCase()];
      if (appleCandidate != null &&
          (platform == 'ios' || platform == 'macos')) {
        await _methods.invokeMethod<String>(
          'connectApplePeerToPeer',
          <String, Object>{'candidateHandle': appleCandidate},
        );
      } else {
        if (endpoints.isEmpty || !hasEligibleLocalNetworkPath) return;
        await pairingConnect(
          sessionId: sessionId,
          peerPresenceId: peer.presenceId,
          endpoints: endpoints,
        );
      }
      await _refreshSnapshot();
    } catch (error) {
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  bool hasConnectablePathFor(DiscoveryPeer peer) {
    final hasAppleCandidate =
        (platform == 'ios' || platform == 'macos') &&
        _appleP2PCandidatesByPresence.containsKey(
          peer.presenceId.toLowerCase(),
        );
    final hasLanEndpoint =
        hasEligibleLocalNetworkPath &&
        (peer.bestEndpoint != null || peer.candidateEndpoints.isNotEmpty);
    return hasAppleCandidate || hasLanEndpoint;
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

  AuthenticatedSessionInfo? lanSessionForPeer(DiscoveryPeer peer) {
    for (final session in authenticatedLanSessions) {
      if (session.peerPresenceId == peer.presenceId) return session;
    }
    return null;
  }

  TransferEvent? transferForSession(BigInt authenticatedSessionId) {
    for (final event in transferActivity.reversed) {
      if (event.authenticatedSessionId == authenticatedSessionId) return event;
    }
    return null;
  }

  TransferEvent? get incomingTransferOffer {
    final latestByTransfer = <String, TransferEvent>{};
    for (final event in transferActivity) {
      latestByTransfer[event.transferId] = event;
    }
    for (final event in latestByTransfer.values.toList().reversed) {
      if (event.kind == TransferEventKind.offerReceived &&
          event.requestId != null &&
          !_respondedTransferRequests.contains(event.requestId)) {
        return event;
      }
    }
    return null;
  }

  Future<void> sendFileToPeer(DiscoveryPeer peer) async {
    final pairingSessionId = _pairingSessionId;
    final lanSession = lanSessionForPeer(peer);
    if (pairingSessionId == null || lanSession == null) return;
    String? stagedPath;
    try {
      final raw = await _identityMethods.invokeMethod<Object?>(
        'pickTransferFile',
      );
      if (raw is! Map<Object?, Object?>) return;
      stagedPath = raw['path'] as String?;
      final advertisedName = raw['name'] as String?;
      if (stagedPath == null ||
          stagedPath.isEmpty ||
          advertisedName == null ||
          advertisedName.isEmpty) {
        throw StateError(
          'Platform file selection did not return a usable file',
        );
      }
      final transferId = await pairingTransferSendFile(
        sessionId: pairingSessionId,
        authenticatedSessionId: lanSession.sessionId,
        sourcePath: stagedPath,
        advertisedName: advertisedName,
      );
      _outgoingSourcesByTransfer[transferId] = stagedPath;
      await _refreshSnapshot();
    } catch (error) {
      if (stagedPath != null) await _discardOutgoingSource(stagedPath);
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> respondToTransfer(BigInt requestId, bool accepted) async {
    final sessionId = _pairingSessionId;
    if (sessionId == null || !_respondedTransferRequests.add(requestId)) return;
    notifyListeners();
    try {
      String? staging;
      String? destination;
      if (accepted) {
        final raw = await _identityMethods.invokeMethod<Object?>(
          'transferDirectories',
        );
        if (raw is! Map<Object?, Object?>) {
          throw StateError('Platform transfer storage is unavailable');
        }
        staging = raw['staging'] as String?;
        destination = raw['destination'] as String?;
        if (staging == null || destination == null) {
          throw StateError('Platform transfer directories are unavailable');
        }
      }
      await pairingTransferRespond(
        sessionId: sessionId,
        requestId: requestId,
        accepted: accepted,
        stagingDirectory: staging,
        destinationDirectory: destination,
      );
      await _refreshSnapshot();
    } catch (error) {
      _respondedTransferRequests.remove(requestId);
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> cancelTransfer(String transferId) async {
    final sessionId = _pairingSessionId;
    if (sessionId == null) return;
    try {
      await pairingTransferCancel(sessionId: sessionId, transferId: transferId);
      await _refreshSnapshot();
    } catch (error) {
      state = DiscoveryRunState.degraded;
      _setNotice(DiscoveryNotice.rustRejected, detail: _safeError(error));
      notifyListeners();
    }
  }

  Future<void> _discardFinishedOutgoingSources(
    List<TransferEvent> events,
  ) async {
    for (final event in events) {
      if (event.direction != TransferDirection.sending ||
          (event.kind != TransferEventKind.completed &&
              event.kind != TransferEventKind.rejected &&
              event.kind != TransferEventKind.cancelled &&
              event.kind != TransferEventKind.failed)) {
        continue;
      }
      final path = _outgoingSourcesByTransfer.remove(event.transferId);
      if (path != null) await _discardOutgoingSource(path);
    }
  }

  Future<void> _discardOutgoingSource(String path) async {
    try {
      await _identityMethods.invokeMethod<void>(
        'discardTransferSource',
        <String, Object>{'path': path},
      );
    } catch (_) {
      // Private staging cleanup is retried by platform maintenance later.
    }
  }

  Future<void> _discardOutgoingSources(Iterable<String> paths) async {
    for (final path in paths) {
      await _discardOutgoingSource(path);
    }
  }

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
    final outgoingSources = _outgoingSourcesByTransfer.values.toList(
      growable: false,
    );
    _outgoingSourcesByTransfer.clear();
    _sessionId = null;
    _pairingSessionId = null;
    _snapshotTimer?.cancel();
    _nativeEvents?.cancel();
    if (sessionId != null || pairingSessionId != null) {
      unawaited(_disposeNative(sessionId, pairingSessionId, outgoingSources));
    } else if (outgoingSources.isNotEmpty) {
      unawaited(_discardOutgoingSources(outgoingSources));
    }
    super.dispose();
  }

  Future<void> _disposeNative(
    BigInt? sessionId,
    BigInt? pairingSessionId,
    List<String> outgoingSources,
  ) async {
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
      if (pairingSessionId != null) {
        await pairingStop(sessionId: pairingSessionId);
      }
    } catch (_) {
      // Application teardown cannot surface an actionable error.
    } finally {
      await _discardOutgoingSources(outgoingSources);
    }
  }
}

String _detectedPlatform() => Platform.isAndroid
    ? 'android'
    : Platform.isIOS
    ? 'ios'
    : 'macos';
