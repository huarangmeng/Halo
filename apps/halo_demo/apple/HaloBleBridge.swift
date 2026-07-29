import CoreBluetooth
import Foundation
import Network
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
  private let pathMonitor = NWPathMonitor()
  private let pathMonitorQueue = DispatchQueue(label: "org.halo.network-status")
  private var lastBleStateName: String?
  private var wifiState = "temporarily_unavailable"
  private var wifiDetail = "wifi_not_connected"
  private var localNetworkState = "temporarily_unavailable"
  private var localNetworkDetail = "no_local_network_route"

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
    pathMonitor.pathUpdateHandler = { [weak self] path in
      DispatchQueue.main.async {
        self?.updateNetworkState(path)
      }
    }
    pathMonitor.start(queue: pathMonitorQueue)
    methodChannel.setMethodCallHandler { [weak self] call, result in
      self?.handle(call, result: result)
    }
    identityChannel.setMethodCallHandler { [weak self] call, result in
      self?.handleIdentity(call, result: result)
    }
    eventChannel.setStreamHandler(self)
  }

  deinit {
    pathMonitor.cancel()
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
    case "capabilities":
      result(capabilityPayload())
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
    } catch let HaloIdentityStorageError.failed(status) {
      result(FlutterError(
        code: "identity-storage",
        message: "Protected identity storage failed (OSStatus \(status))",
        details: ["osStatus": status]
      ))
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
      // Keep the identity in the app-scoped modern keychain. In particular,
      // never query the legacy login keychain, which can prompt for its
      // password, and never opt the identity into synchronization.
      kSecUseDataProtectionKeychain: true,
      kSecAttrSynchronizable: false,
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
      throw HaloIdentityStorageError.failed(status)
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
      throw HaloIdentityStorageError.failed(updateStatus)
    }
    var item = identityQuery()
    attributes.forEach { item[$0.key] = $0.value }
    let addStatus = SecItemAdd(item as CFDictionary, nil)
    guard addStatus == errSecSuccess else {
      throw HaloIdentityStorageError.failed(addStatus)
    }
  }

  private func deleteIdentity() throws {
    let status = SecItemDelete(identityQuery() as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw HaloIdentityStorageError.failed(status)
    }
  }

  private func trustStoreDirectory() throws -> URL {
    guard let applicationSupport = FileManager.default.urls(
      for: .applicationSupportDirectory,
      in: .userDomainMask
    ).first else {
      throw HaloIdentityStorageError.failed(errSecNotAvailable)
    }
    return applicationSupport
      .appendingPathComponent(Bundle.main.bundleIdentifier ?? "org.halo", isDirectory: true)
      .appendingPathComponent("halo-trust-v1", isDirectory: true)
  }

  private func emit(_ event: HaloBleEvent, generation: UInt64) {
    let payload: [String: Any]
    switch event {
    case .state(let state):
      payload = [
        "type": "state",
        "state": stateName(state),
        "detail": stateDetail(state),
      ]
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
      if case .state(let state) = event {
        lastBleStateName = stateName(state)
      }
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

  private func stateDetail(_ state: HaloBleState) -> String {
    switch state {
    case .starting: "ble_starting"
    case .ready: "ble_ready"
    case .poweredOff: "bluetooth_powered_off"
    case .unauthorized: "bluetooth_permission_denied"
    case .unsupported: "ble_unsupported"
    case .resetting: "bluetooth_resetting"
    case .stopped: "ble_stopped"
    }
  }

  private func capabilityPayload() -> [[String: String]] {
    let bluetooth: [String: String]
    if let state = lastBleStateName {
      bluetooth = capability(
        "bluetooth",
        state,
        state == "hardware_off" ? "bluetooth_powered_off" : "bluetooth_\(state)"
      )
    } else {
      switch CBManager.authorization {
      case .denied, .restricted:
        bluetooth = capability(
          "bluetooth",
          "permission_denied",
          "bluetooth_permission_denied"
        )
      case .notDetermined:
        bluetooth = capability(
          "bluetooth",
          "permission_required",
          "bluetooth_permission_not_requested"
        )
      case .allowedAlways:
        bluetooth = capability(
          "bluetooth",
          "starting",
          "bluetooth_state_checked_when_discovery_starts"
        )
      @unknown default:
        bluetooth = capability("bluetooth", "unsupported", "ble_unsupported")
      }
    }

    #if os(macOS)
    let background = capability("background", "ready", "application_process_background")
    #else
    let background = capability("background", "unsupported", "foreground_only")
    #endif

    return [
      bluetooth,
      capability("wifi", wifiState, wifiDetail),
      capability("local_network", localNetworkState, localNetworkDetail),
      background,
    ]
  }

  private func updateNetworkState(_ path: NWPath) {
    guard path.status == .satisfied else {
      wifiState = "temporarily_unavailable"
      wifiDetail = "wifi_not_connected"
      localNetworkState = "temporarily_unavailable"
      localNetworkDetail = "no_local_network_route"
      return
    }
    if path.usesInterfaceType(.wifi) {
      wifiState = "ready"
      wifiDetail = "wifi_connected"
      localNetworkState = "ready"
      localNetworkDetail = "local_network_connected"
    } else if path.usesInterfaceType(.wiredEthernet) {
      wifiState = "temporarily_unavailable"
      wifiDetail = "wifi_not_connected"
      localNetworkState = "ready"
      localNetworkDetail = "ethernet_connected"
    } else {
      wifiState = "temporarily_unavailable"
      wifiDetail = "wifi_not_connected"
      localNetworkState = "temporarily_unavailable"
      localNetworkDetail = "no_local_network_route"
    }
  }

  private func capability(_ name: String, _ state: String, _ detail: String) -> [String: String] {
    ["name": name, "state": state, "detail": detail]
  }
}

private enum HaloIdentityStorageError: Error {
  case failed(OSStatus)
}
