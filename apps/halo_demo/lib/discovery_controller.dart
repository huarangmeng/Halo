import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'src/rust/api.dart';

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
  macosBluetoothPermissionDenied,
  starting,
  running,
  nativeEventStopped,
  startFailed,
  cleanupFailed,
  bleUnavailable,
  diagnostic,
  rustRejected,
}

class DiscoveryController extends ChangeNotifier {
  static const _methods = MethodChannel('org.halo.discovery/ble');
  static const _events = EventChannel('org.halo.discovery/ble-events');

  DiscoveryRunState state = DiscoveryRunState.stopped;
  DiscoveryNotice notice = DiscoveryNotice.stopped;
  String? noticeDetail;
  String? noticeOperation;
  String? localPresenceId;
  DiscoveryDeviceType? localDeviceType;
  List<DiscoveryPeer> peers = const [];

  BigInt? _sessionId;
  StreamSubscription<Object?>? _nativeEvents;
  Timer? _snapshotTimer;
  Future<void>? _startOperation;
  Future<void>? _stopOperation;
  bool _stopRequested = false;

  bool get canStart =>
      _startOperation == null &&
      _stopOperation == null &&
      (state == DiscoveryRunState.stopped || state == DiscoveryRunState.failed);
  bool get canStop => _sessionId != null;
  bool get hasActiveWork =>
      state != DiscoveryRunState.stopped || _startOperation != null;
  bool get isRunning =>
      state == DiscoveryRunState.running || state == DiscoveryRunState.degraded;
  String get platform => Platform.isAndroid ? 'android' : 'macos';
  DiscoveryDeviceType get platformDeviceType => Platform.isAndroid
      ? DiscoveryDeviceType.android
      : DiscoveryDeviceType.macos;

  Future<void> start() {
    if (!canStart) return Future<void>.value();
    _stopRequested = false;
    final operation = _start();
    _startOperation = operation;
    return operation.whenComplete(() {
      if (identical(_startOperation, operation)) _startOperation = null;
    });
  }

  Future<void> _start() async {
    state = DiscoveryRunState.preparing;
    _setNotice(DiscoveryNotice.permissionContext);
    notifyListeners();

    try {
      final preparation = await _methods.invokeMethod<Object?>('prepare');
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
          'permission_denied' when Platform.isMacOS =>
            DiscoveryNotice.macosBluetoothPermissionDenied,
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
      final bootstrap = await discoveryStart(
        quicPort: 4433,
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
    _sessionId = null;
    _snapshotTimer?.cancel();
    _snapshotTimer = null;
    await _nativeEvents?.cancel();
    _nativeEvents = null;
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
    } catch (error) {
      state = DiscoveryRunState.failed;
      _setNotice(DiscoveryNotice.cleanupFailed, detail: _safeError(error));
      notifyListeners();
      return;
    }
    peers = const [];
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
        await discoveryReportBleState(
          sessionId: sessionId,
          platform: platform,
          state: _providerState(rawState),
        );
        if (rawState == 'ready') {
          state = DiscoveryRunState.running;
          _setNotice(DiscoveryNotice.running);
        } else if (rawState != 'starting') {
          state = DiscoveryRunState.degraded;
          _setNotice(
            rawState == 'permission_denied' && Platform.isMacOS
                ? DiscoveryNotice.macosBluetoothPermissionDenied
                : DiscoveryNotice.bleUnavailable,
            detail: rawState,
          );
        }
        notifyListeners();
      } else if (type == 'diagnostic') {
        _setNotice(
          DiscoveryNotice.diagnostic,
          operation: '${rawEvent['operation']}',
          detail: '${rawEvent['detail']}',
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
    if (sessionId == null) return;
    try {
      peers = await discoverySnapshot(sessionId: sessionId);
      notifyListeners();
    } catch (_) {
      // A simultaneous stop closes the session; stop owns the visible state.
    }
  }

  Future<void> _cleanupAfterFailedStart() async {
    _snapshotTimer?.cancel();
    _snapshotTimer = null;
    await _nativeEvents?.cancel();
    _nativeEvents = null;
    final sessionId = _sessionId;
    _sessionId = null;
    localPresenceId = null;
    localDeviceType = null;
    try {
      await _methods.invokeMethod<void>('stop');
      if (sessionId != null) await discoveryStop(sessionId: sessionId);
    } catch (_) {
      // Preserve the original start failure.
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
    _sessionId = null;
    _snapshotTimer?.cancel();
    _nativeEvents?.cancel();
    if (sessionId != null) unawaited(_disposeNative(sessionId));
    super.dispose();
  }

  Future<void> _disposeNative(BigInt sessionId) async {
    try {
      await _methods.invokeMethod<void>('stop');
      await discoveryStop(sessionId: sessionId);
    } catch (_) {
      // Application teardown cannot surface an actionable error.
    }
  }
}
