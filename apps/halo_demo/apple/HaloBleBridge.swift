import CoreBluetooth
import Foundation

#if os(macOS)
import FlutterMacOS
#else
import Flutter
#endif

/// Shared Flutter-to-CoreBluetooth bridge for the iOS and macOS launchers.
///
/// The bridge only manages platform lifecycle and forwards opaque Presence
/// bytes. Rust remains the sole parser and discovery state owner.
final class HaloBleBridge: NSObject, FlutterStreamHandler, @unchecked Sendable {
  private let methodChannel: FlutterMethodChannel
  private let eventChannel: FlutterEventChannel
  private var eventSink: FlutterEventSink?
  private var provider: HaloBleProvider?
  private var providerGeneration: UInt64 = 0

  init(messenger: FlutterBinaryMessenger) {
    methodChannel = FlutterMethodChannel(
      name: "org.halo.discovery/ble",
      binaryMessenger: messenger
    )
    eventChannel = FlutterEventChannel(
      name: "org.halo.discovery/ble-events",
      binaryMessenger: messenger
    )
    super.init()
    methodChannel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result: result)
    }
    eventChannel.setStreamHandler(self)
  }

  deinit {
    providerGeneration &+= 1
    provider?.stopAndWait()
  }

  func onListen(
    withArguments arguments: Any?,
    eventSink events: @escaping FlutterEventSink
  ) -> FlutterError? {
    eventSink = events
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    eventSink = nil
    return nil
  }

  private func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "prepare":
      switch CBManager.authorization {
      case .denied, .restricted:
        result(["ready": false, "reason": "permission_denied"])
      case .allowedAlways, .notDetermined:
        result(["ready": true, "reason": "ready"])
      @unknown default:
        result(["ready": false, "reason": "permission_denied"])
      }
    case "start":
      guard let presence = presenceArgument(call) else {
        result(FlutterError(
          code: "invalid-presence",
          message: "Rust must supply exactly 58 bytes",
          details: nil
        ))
        return
      }
      do {
        let configuration = try HaloBleConfiguration(presence: presence)
        providerGeneration &+= 1
        let generation = providerGeneration
        provider?.stopAndWait()
        provider = HaloBleProvider(
          configuration: configuration,
          eventHandler: { [weak self] event in
            self?.emit(event, generation: generation)
          },
          wakeLanHandler: { _ in 1 }
        )
        provider?.start()
        result(nil)
      } catch {
        result(FlutterError(
          code: "ble-start-failed",
          message: String(describing: error),
          details: nil
        ))
      }
    case "updatePresence":
      guard let presence = presenceArgument(call) else {
        result(FlutterError(
          code: "invalid-presence",
          message: "Rust must supply exactly 58 bytes",
          details: nil
        ))
        return
      }
      do {
        try provider?.updatePresence(presence)
        result(nil)
      } catch {
        result(FlutterError(
          code: "ble-update-failed",
          message: String(describing: error),
          details: nil
        ))
      }
    case "stop":
      providerGeneration &+= 1
      provider?.stopAndWait()
      provider = nil
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  private func presenceArgument(_ call: FlutterMethodCall) -> Data? {
    guard
      let arguments = call.arguments as? [String: Any],
      let typedData = arguments["presence"] as? FlutterStandardTypedData,
      typedData.data.count == HaloBleConfiguration.presenceLength
    else {
      return nil
    }
    return typedData.data
  }

  private func emit(_ event: HaloBleEvent, generation: UInt64) {
    let payload: [String: Any]
    switch event {
    case .state(let state):
      payload = ["type": "state", "state": stateName(state)]
    case .presence(_, let descriptor, let rssi):
      payload = [
        "type": "presence",
        "descriptor": FlutterStandardTypedData(bytes: descriptor),
        "rssi": rssi,
      ]
    case .diagnostic(let diagnostic):
      payload = [
        "type": "diagnostic",
        "operation": "corebluetooth",
        "detail": String(describing: diagnostic),
      ]
    }
    DispatchQueue.main.async { [weak self] in
      guard let self, generation == providerGeneration else { return }
      eventSink?(payload)
    }
  }

  private func stateName(_ state: HaloBleState) -> String {
    switch state {
    case .starting: "starting"
    case .ready: "ready"
    case .poweredOff: "hardware_off"
    case .unauthorized: "permission_denied"
    case .unsupported: "unsupported"
    case .resetting: "temporarily_unavailable"
    case .stopped: "stopped"
    }
  }
}
