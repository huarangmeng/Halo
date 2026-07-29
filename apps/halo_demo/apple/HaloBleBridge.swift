import CoreBluetooth
import Foundation
import Security

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
  private let identityChannel: FlutterMethodChannel
  private let eventChannel: FlutterEventChannel
  private var eventSink: FlutterEventSink?
  private var provider: HaloBleProvider?
  private var providerGeneration: UInt64 = 0

  init(messenger: FlutterBinaryMessenger) {
    methodChannel = FlutterMethodChannel(
      name: "org.halo.discovery/ble",
      binaryMessenger: messenger
    )
    identityChannel = FlutterMethodChannel(
      name: "org.halo.identity/storage",
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
    identityChannel.setMethodCallHandler { [weak self] call, result in
      self?.handleIdentity(call, result: result)
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

  private func handleIdentity(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    do {
      switch call.method {
      case "load":
        if let data = try loadIdentity() {
          result(FlutterStandardTypedData(bytes: data))
        } else {
          result(nil)
        }
      case "save":
        guard
          let arguments = call.arguments as? [String: Any],
          let typedData = arguments["blob"] as? FlutterStandardTypedData,
          !typedData.data.isEmpty,
          typedData.data.count <= 256
        else {
          result(FlutterError(
            code: "invalid-identity",
            message: "Rust identity blob length is invalid",
            details: nil
          ))
          return
        }
        try saveIdentity(typedData.data)
        result(nil)
      case "delete":
        try deleteIdentity()
        result(nil)
      case "trustStoreDirectory":
        result(try trustStoreDirectory().path)
      default:
        result(FlutterMethodNotImplemented)
      }
    } catch {
      result(FlutterError(
        code: "identity-storage",
        message: "Protected identity storage failed",
        details: nil
      ))
    }
  }

  private func identityQuery() -> [CFString: Any] {
    [
      kSecClass: kSecClassGenericPassword,
      kSecAttrService: "org.halo.identity",
      kSecAttrAccount: "device-identity-v1",
    ]
  }

  private func loadIdentity() throws -> Data? {
    var query = identityQuery()
    query[kSecReturnData] = true
    query[kSecMatchLimit] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let data = item as? Data,
          !data.isEmpty, data.count <= 256 else {
      throw HaloIdentityStorageError.failed
    }
    return data
  }

  private func saveIdentity(_ data: Data) throws {
    let attributes: [CFString: Any] = [
      kSecValueData: data,
      kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    let updateStatus = SecItemUpdate(
      identityQuery() as CFDictionary,
      attributes as CFDictionary
    )
    if updateStatus == errSecSuccess { return }
    guard updateStatus == errSecItemNotFound else {
      throw HaloIdentityStorageError.failed
    }
    var item = identityQuery()
    attributes.forEach { item[$0.key] = $0.value }
    guard SecItemAdd(item as CFDictionary, nil) == errSecSuccess else {
      throw HaloIdentityStorageError.failed
    }
  }

  private func deleteIdentity() throws {
    let status = SecItemDelete(identityQuery() as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw HaloIdentityStorageError.failed
    }
  }

  private func trustStoreDirectory() throws -> URL {
    guard let applicationSupport = FileManager.default.urls(
      for: .applicationSupportDirectory,
      in: .userDomainMask
    ).first else {
      throw HaloIdentityStorageError.failed
    }
    return applicationSupport
      .appendingPathComponent(Bundle.main.bundleIdentifier ?? "org.halo", isDirectory: true)
      .appendingPathComponent("halo-trust-v1", isDirectory: true)
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

private enum HaloIdentityStorageError: Error {
  case failed
}
