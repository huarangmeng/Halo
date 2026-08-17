import CoreBluetooth
import Foundation
import Network
import Security
import halo_ffi

#if os(macOS)
import AppKit
import FlutterMacOS
#else
import Flutter
#endif

/// Shared Flutter-to-Apple-networking bridge for the iOS and macOS launchers.
///
/// The bridge manages platform lifecycle, forwards opaque BLE Presence bytes,
/// and joins native Apple QUIC streams directly to Rust. Dart never receives
/// exporter material, control frames, or file bytes.
final class HaloBleBridge: NSObject, FlutterStreamHandler, @unchecked Sendable {
  private let methodChannel: FlutterMethodChannel
  private let identityChannel: FlutterMethodChannel
  private let eventChannel: FlutterEventChannel
  private var eventSink: FlutterEventSink?
  private var provider: HaloBleProvider?
  private var applePeerToPeerProvider: HaloApplePeerToPeerProvider?
  private var applePeerToPeerChannels: [UUID: HaloAppleQuicControlChannel] = [:]
  private var applePeerToPeerDataStreams: [UUID: HaloAppleQuicDataStream] = [:]
  private var applePeerToPeerPairingTasks: [UUID: Task<Void, Never>] = [:]
  private var pairingSessionID: UInt64?
  private var providerGeneration: UInt64 = 0
  private let pathMonitor = NWPathMonitor()
  private let pathMonitorQueue = DispatchQueue(label: "org.halo.network-status")
  private var latestNetworkPath: NWPath?
  private var preparedLanInterfaceIndex: UInt32?
  private var lanPreparationAttempted = false
  private var lanScope: HaloAppleLocalNetworkScope = .shared
  private var localHotspotDetail = "local_hotspot_not_joined"
  private var lastBleStateName: String?
  private var wifiState = "temporarily_unavailable"
  private var wifiDetail = "wifi_not_connected"
  private var localNetworkState = "temporarily_unavailable"
  private var localNetworkDetail = "no_local_network_route"
  private var applePeerToPeerState = "stopped"
  private var applePeerToPeerDetail = "apple_p2p_stopped"

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
    applePeerToPeerProvider?.stopAndWait()
    let channels = applePeerToPeerChannels.values
    Task { for channel in channels { await channel.close() } }
    for task in applePeerToPeerPairingTasks.values { task.cancel() }
    let dataStreams = applePeerToPeerDataStreams.values
    Task { for stream in dataStreams { await stream.close() } }
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
        prepareLanSocket()
        result(["ready": true, "reason": "ready"])
      @unknown default:
        result(["ready": false, "reason": "permission_denied"])
      }
    case "capabilities":
      result(capabilityPayload())
    case "joinLocalHotspot":
      #if os(macOS)
      guard provider == nil else {
        localHotspotDetail = "local_hotspot_stop_discovery_first"
        result(["ready": false, "detail": localHotspotDetail])
        return
      }
      lanScope = .userApprovedHotspot
      localHotspotDetail = "local_hotspot_joining"
      prepareLanSocket()
      result([
        "ready": localNetworkState == "ready",
        "detail": localHotspotDetail,
      ])
      #else
      result(FlutterMethodNotImplemented)
      #endif
    case "leaveLocalHotspot":
      #if os(macOS)
      guard provider == nil else {
        localHotspotDetail = "local_hotspot_stop_discovery_first"
        result(["ready": false, "detail": localHotspotDetail])
        return
      }
      lanScope = .shared
      localHotspotDetail = "local_hotspot_not_joined"
      prepareLanSocket()
      result(["ready": true, "detail": localHotspotDetail])
      #else
      result(FlutterMethodNotImplemented)
      #endif
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
        startApplePeerToPeer(call, generation: generation)
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
      stopApplePeerToPeer()
      result(nil)
    case "connectApplePeerToPeer":
      guard let candidateHandle = uuidArgument(call, key: "candidateHandle"),
            let linkHandle = applePeerToPeerProvider?.connect(candidateHandle: candidateHandle)
      else {
        result(FlutterError(
          code: "apple-p2p-candidate-unavailable",
          message: "The Apple P2P candidate is no longer available",
          details: nil
        ))
        return
      }
      result(linkHandle.uuidString)
    case "applePeerToPeerClose":
      guard let handle = uuidArgument(call, key: "linkHandle") else {
        result(FlutterError(code: "apple-p2p-invalid-link", message: nil, details: nil))
        return
      }
      let channel = applePeerToPeerChannels.removeValue(forKey: handle)
      applePeerToPeerProvider?.cancelConnection(handle: handle)
      Task {
        if let channel { await channel.close() }
        await MainActor.run { result(nil) }
      }
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

  private func dataArgument(_ call: FlutterMethodCall, key: String) -> Data? {
    guard let arguments = call.arguments as? [String: Any],
          let typedData = arguments[key] as? FlutterStandardTypedData
    else {
      return nil
    }
    return typedData.data
  }

  private func uuidArgument(_ call: FlutterMethodCall, key: String) -> UUID? {
    guard let arguments = call.arguments as? [String: Any],
          let value = arguments[key] as? String
    else {
      return nil
    }
    return UUID(uuidString: value)
  }

  private func startApplePeerToPeer(_ call: FlutterMethodCall, generation: UInt64) {
    do {
      guard let arguments = call.arguments as? [String: Any],
            let instanceName = arguments["p2pInstanceName"] as? String,
            let pairingSessionNumber = arguments["pairingSessionId"] as? NSNumber,
            pairingSessionNumber.uint64Value != 0,
            let certificate = dataArgument(call, key: "platformTlsCertificateDer"),
            let privateKey = dataArgument(call, key: "platformTlsPrivateKeyX963")
      else {
        throw HaloAppleQuicTlsIdentityError.invalidCertificate
      }
      let identity = try HaloAppleQuicTlsIdentity(
        certificateDER: certificate,
        privateKeyX963: privateKey
      )
      let configuration = try HaloApplePeerToPeerConfiguration(instanceName: instanceName)
      pairingSessionID = pairingSessionNumber.uint64Value
      applePeerToPeerProvider?.stopAndWait()
      let p2pProvider = HaloApplePeerToPeerProvider(
        configuration: configuration,
        parametersFactory: {
          HaloApplePeerToPeerNetworkPolicy.makeQuicParameters { options in
            identity.configure(options)
          }
        },
        eventHandler: { [weak self] event in
          self?.emitApplePeerToPeer(event, generation: generation)
        }
      )
      applePeerToPeerProvider = p2pProvider
      p2pProvider.start()
    } catch {
      applePeerToPeerState = "failed"
      applePeerToPeerDetail = "apple_p2p_identity_failed"
      emitPayload([
        "type": "data_channel_state",
        "kind": "apple_peer_to_peer",
        "state": "failed",
        "detail": applePeerToPeerDetail,
      ], generation: generation)
    }
  }

  private func stopApplePeerToPeer() {
    applePeerToPeerProvider?.stopAndWait()
    applePeerToPeerProvider = nil
    let channels = applePeerToPeerChannels.values
    applePeerToPeerChannels.removeAll(keepingCapacity: false)
    for task in applePeerToPeerPairingTasks.values { task.cancel() }
    applePeerToPeerPairingTasks.removeAll(keepingCapacity: false)
    Task { for channel in channels { await channel.close() } }
    let dataStreams = applePeerToPeerDataStreams.values
    applePeerToPeerDataStreams.removeAll(keepingCapacity: false)
    Task { for stream in dataStreams { await stream.close() } }
    pairingSessionID = nil
    applePeerToPeerState = "stopped"
    applePeerToPeerDetail = "apple_p2p_stopped"
  }

  private func emitApplePeerToPeer(
    _ event: HaloApplePeerToPeerEvent,
    generation: UInt64
  ) {
    switch event {
    case .state(let state):
      let stateName = applePeerToPeerStateName(state)
      emitPayload([
        "type": "data_channel_state",
        "kind": "apple_peer_to_peer",
        "state": stateName,
        "detail": "apple_p2p_\(stateName)",
      ], generation: generation) { [weak self] in
        self?.applePeerToPeerState = stateName
        self?.applePeerToPeerDetail = "apple_p2p_\(stateName)"
      }
    case .candidateFound(let handle, let peerPresenceID):
      emitPayload([
        "type": "data_channel_candidate",
        "kind": "apple_peer_to_peer",
        "action": "found",
        "handle": handle.uuidString,
        "peerPresenceId": peerPresenceID.uuidString.lowercased(),
      ], generation: generation)
    case .candidateLost(let handle, let peerPresenceID):
      emitPayload([
        "type": "data_channel_candidate",
        "kind": "apple_peer_to_peer",
        "action": "lost",
        "handle": handle.uuidString,
        "peerPresenceId": peerPresenceID.uuidString.lowercased(),
      ], generation: generation)
    case .linkReady(let handle, let direction, let peerPresenceID):
      do {
        guard let sessionID = pairingSessionID,
              let channel = try applePeerToPeerProvider?.takeControlChannel(handle: handle)
        else { throw HaloAppleQuicControlError.closed }
        var payload: [String: Any] = [
          "type": "data_channel_link_ready",
          "kind": "apple_peer_to_peer",
          "handle": handle.uuidString,
          "direction": direction.rawValue,
        ]
        if let peerPresenceID {
          payload["peerPresenceId"] = peerPresenceID.uuidString.lowercased()
        }
        emitPayload(payload, generation: generation) { [weak self] in
          self?.startApplePeerToPeerPairing(
            handle: handle,
            channel: channel,
            sessionID: sessionID,
            direction: direction,
            peerPresenceID: peerPresenceID,
            generation: generation
          )
        }
      } catch {
        emitPayload([
          "type": "diagnostic",
          "operation": "apple_peer_to_peer",
          "detail": "control_channel_unavailable",
        ], generation: generation)
      }
    case .linkFailed(let handle, let failure):
      emitPayload([
        "type": "data_channel_link_failed",
        "kind": "apple_peer_to_peer",
        "handle": handle.uuidString,
        "detail": failure.rawValue,
      ], generation: generation)
    case .dataStreamReady(let handle, let sessionHandle, let direction):
      do {
        guard let stream = try applePeerToPeerProvider?.takeDataStream(handle: handle)
        else { throw HaloAppleQuicDataError.closed }
        emitPayload([
          "type": "data_channel_stream_ready",
          "kind": "apple_peer_to_peer",
          "handle": handle.uuidString,
          "sessionHandle": sessionHandle.uuidString,
          "direction": direction.rawValue,
        ], generation: generation) { [weak self] in
          self?.applePeerToPeerDataStreams[handle] = stream
        }
      } catch {
        emitPayload([
          "type": "diagnostic",
          "operation": "apple_peer_to_peer_data",
          "detail": "data_stream_unavailable",
        ], generation: generation)
      }
    case .dataStreamFailed(let handle, let failure):
      emitPayload([
        "type": "data_channel_stream_failed",
        "kind": "apple_peer_to_peer",
        "handle": handle.uuidString,
        "detail": failure.rawValue,
      ], generation: generation)
    case .diagnostic(let failure):
      emitPayload([
        "type": "diagnostic",
        "operation": "apple_peer_to_peer",
        "detail": failure.rawValue,
      ], generation: generation)
    }
  }

  private func startApplePeerToPeerPairing(
    handle: UUID,
    channel: HaloAppleQuicControlChannel,
    sessionID: UInt64,
    direction: HaloApplePeerToPeerDirection,
    peerPresenceID: UUID?,
    generation: UInt64
  ) {
    guard applePeerToPeerPairingTasks[handle] == nil else { return }
    applePeerToPeerChannels[handle] = channel
    applePeerToPeerPairingTasks[handle] = Task { [weak self] in
      let outcome = await haloRunApplePairingBridge(
        sessionID: sessionID,
        channel: channel,
        direction: direction,
        peerPresenceID: peerPresenceID
      )
      await channel.close()
      await MainActor.run { [weak self] in
        guard let self else { return }
        applePeerToPeerPairingTasks.removeValue(forKey: handle)
        applePeerToPeerChannels.removeValue(forKey: handle)
        let authenticated = outcome == .authenticated
          && pairingSessionID == sessionID
          && generation == providerGeneration
        applePeerToPeerProvider?.finishPairing(
          handle: handle,
          authenticated: authenticated,
          channelBinding: authenticated ? channel.channelBinding : nil
        )
        if authenticated {
          emitPayload([
            "type": "data_channel_session_ready",
            "kind": "apple_peer_to_peer",
            "handle": handle.uuidString,
          ], generation: generation)
        } else if outcome != .cancelled {
          emitPayload([
            "type": "diagnostic",
            "operation": "apple_peer_to_peer",
            "detail": "native_pairing_bridge_failed",
          ], generation: generation)
        }
      }
    }
  }

  private func emitPayload(
    _ payload: [String: Any],
    generation: UInt64,
    beforeEmit: (@Sendable () -> Void)? = nil
  ) {
    DispatchQueue.main.async { [weak self] in
      guard let self, generation == providerGeneration else { return }
      beforeEmit?()
      eventSink?(payload)
    }
  }

  private func applePeerToPeerStateName(_ state: HaloApplePeerToPeerState) -> String {
    switch state {
    case .starting: "starting"
    case .ready: "ready"
    case .temporarilyUnavailable: "temporarily_unavailable"
    case .failed: "failed"
    case .stopped: "stopped"
    }
  }

  private func handleIdentity(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    if call.method == "transferDirectories" {
      do {
        result(try transferDirectories())
      } catch {
        result(FlutterError(
          code: "transfer-storage",
          message: "Private transfer storage is unavailable",
          details: nil
        ))
      }
      return
    }
    if call.method == "pickTransferFile" {
      pickTransferFile(result: result)
      return
    }
    if call.method == "discardTransferSource" {
      do {
        guard let arguments = call.arguments as? [String: Any],
              let path = arguments["path"] as? String else {
          throw CocoaError(.fileNoSuchFile)
        }
        try discardTransferSource(path: path)
        result(nil)
      } catch {
        result(FlutterError(
          code: "transfer-storage",
          message: "Private transfer source could not be removed",
          details: nil
        ))
      }
      return
    }
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

  private func transferRootDirectory() throws -> URL {
    guard let applicationSupport = FileManager.default.urls(
      for: .applicationSupportDirectory,
      in: .userDomainMask
    ).first else {
      throw HaloIdentityStorageError.failed(errSecNotAvailable)
    }
    return applicationSupport
      .appendingPathComponent(Bundle.main.bundleIdentifier ?? "org.halo", isDirectory: true)
      .appendingPathComponent("halo-transfer-v1", isDirectory: true)
  }

  private func transferDirectories() throws -> [String: String] {
    let root = try transferRootDirectory()
    let staging = root.appendingPathComponent("staging", isDirectory: true)
    let destination = root.appendingPathComponent("received", isDirectory: true)
    let outgoing = root.appendingPathComponent("outgoing", isDirectory: true)
    for directory in [staging, destination, outgoing] {
      try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: true
      )
    }
    return ["staging": staging.path, "destination": destination.path]
  }

  private func pickTransferFile(result: @escaping FlutterResult) {
    #if os(macOS)
    let panel = NSOpenPanel()
    panel.canChooseFiles = true
    panel.canChooseDirectories = false
    panel.allowsMultipleSelection = false
    panel.begin { [weak self] response in
      guard response == .OK, let source = panel.url else {
        result(nil)
        return
      }
      guard let self else {
        result(FlutterError(code: "transfer-storage", message: nil, details: nil))
        return
      }
      let accessed = source.startAccessingSecurityScopedResource()
      defer { if accessed { source.stopAccessingSecurityScopedResource() } }
      do {
        let values = try source.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values.isRegularFile == true,
              Int64(values.fileSize ?? 0) <= 10 * 1024 * 1024 * 1024 * 1024 else {
          throw CocoaError(.fileReadTooLarge)
        }
        let root = try transferRootDirectory()
        let outgoing = root.appendingPathComponent("outgoing", isDirectory: true)
        try FileManager.default.createDirectory(
          at: outgoing,
          withIntermediateDirectories: true
        )
        let destination = outgoing
          .appendingPathComponent(UUID().uuidString)
          .appendingPathExtension("upload")
        try FileManager.default.copyItem(at: source, to: destination)
        result(["path": destination.path, "name": source.lastPathComponent])
      } catch {
        result(FlutterError(
          code: "transfer-file-copy",
          message: "Selected file could not be copied into private transfer storage",
          details: nil
        ))
      }
    }
    #else
    result(FlutterError(
      code: "file-picker-unavailable",
      message: "The iOS document picker is not connected yet",
      details: nil
    ))
    #endif
  }

  private func discardTransferSource(path: String) throws {
    let outgoing = try transferRootDirectory()
      .appendingPathComponent("outgoing", isDirectory: true)
      .standardizedFileURL
    let candidate = URL(fileURLWithPath: path).standardizedFileURL
    guard candidate.deletingLastPathComponent() == outgoing,
          candidate.pathExtension == "upload" else {
      throw CocoaError(.fileNoSuchFile)
    }
    if FileManager.default.fileExists(atPath: candidate.path) {
      try FileManager.default.removeItem(at: candidate)
    }
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

    var capabilities = [
      bluetooth,
      capability("wifi", wifiState, wifiDetail),
      capability("local_network", localNetworkState, localNetworkDetail),
      capability(
        "apple_peer_to_peer",
        applePeerToPeerState,
        applePeerToPeerDetail
      ),
      capability("wifi_direct", "unsupported", "wifi_direct_unsupported_on_apple"),
      wifiAwareCapability(),
      background,
    ]
    #if os(macOS)
    capabilities.insert(localHotspotCapability(), at: 3)
    #endif
    return capabilities
  }

  private func localHotspotCapability() -> [String: String] {
    let state: String
    switch localHotspotDetail {
    case "local_hotspot_joined": state = "ready"
    case "local_hotspot_joining": state = "starting"
    case "local_hotspot_not_joined", "local_hotspot_cancelled": state = "stopped"
    case "local_hotspot_unavailable", "local_hotspot_lost",
         "local_hotspot_stop_discovery_first": state = "temporarily_unavailable"
    default: state = "failed"
    }
    return capability("local_hotspot", state, localHotspotDetail)
  }

  private func wifiAwareCapability() -> [String: String] {
    #if os(iOS)
    if #available(iOS 26.0, *) {
      return capability("wifi_aware", "stopped", "wifi_aware_provider_not_implemented")
    }
    return capability("wifi_aware", "unsupported", "wifi_aware_os_unsupported")
    #else
    return capability("wifi_aware", "unsupported", "wifi_aware_unsupported_on_macos")
    #endif
  }

  private func updateNetworkState(_ path: NWPath) {
    latestNetworkPath = path
    defer { synchronizeLocalHotspotState() }
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
    } else if path.usesInterfaceType(.wiredEthernet) {
      wifiState = "temporarily_unavailable"
      wifiDetail = "wifi_not_connected"
    } else {
      wifiState = "temporarily_unavailable"
      wifiDetail = "wifi_not_connected"
      localNetworkState = "temporarily_unavailable"
      localNetworkDetail = "no_local_network_route"
      return
    }

    guard let interface = HaloAppleBoundLanSocket.eligibleInterface(
      on: path,
      scope: lanScope
    ),
          let interfaceIndex = UInt32(exactly: interface.index)
    else {
      localNetworkState = "temporarily_unavailable"
      localNetworkDetail = path.isExpensive
        ? "local_network_metered"
        : path.isConstrained
          ? "local_network_constrained"
          : "no_local_network_route"
      return
    }
    if let preparedLanInterfaceIndex {
      if preparedLanInterfaceIndex == interfaceIndex {
        localNetworkState = "ready"
        localNetworkDetail = "local_network_socket_bound"
      } else {
        localNetworkState = "temporarily_unavailable"
        localNetworkDetail = "local_network_restart_required"
      }
    } else if lanPreparationAttempted {
      localNetworkState = "temporarily_unavailable"
      localNetworkDetail = "local_network_restart_required"
    } else {
      localNetworkState = "stopped"
      localNetworkDetail = "local_network_not_prepared"
    }
  }

  private func prepareLanSocket() {
    lanPreparationAttempted = true
    preparedLanInterfaceIndex = nil
    let path = latestNetworkPath ?? pathMonitor.currentPath
    guard let interface = HaloAppleBoundLanSocket.eligibleInterface(
      on: path,
      scope: lanScope
    ),
          let interfaceIndex = UInt32(exactly: interface.index)
    else {
      if halo_apple_lan_disable() != haloAppleNativeStatusOK {
        localNetworkState = "failed"
        localNetworkDetail = "local_network_binding_failed"
      } else {
        updateNetworkState(path)
      }
      synchronizeLocalHotspotState()
      return
    }
    do {
      let descriptor = try HaloAppleBoundLanSocket.makeIPv4Socket(on: interface)
      // Rust consumes every valid descriptor, including when registration
      // returns an internal error. Swift must never close it after this call.
      let registrationStatus = switch lanScope {
      case .shared:
        halo_apple_lan_register_bound_socket(descriptor)
      case .userApprovedHotspot:
        halo_apple_lan_register_user_approved_hotspot_socket(descriptor)
      }
      guard registrationStatus == haloAppleNativeStatusOK else {
        localNetworkState = "failed"
        localNetworkDetail = "local_network_binding_failed"
        synchronizeLocalHotspotState()
        return
      }
      preparedLanInterfaceIndex = interfaceIndex
      localNetworkState = "ready"
      localNetworkDetail = "local_network_socket_bound"
      synchronizeLocalHotspotState()
    } catch {
      _ = halo_apple_lan_disable()
      localNetworkState = "failed"
      localNetworkDetail = "local_network_binding_failed"
      synchronizeLocalHotspotState()
    }
  }

  private func synchronizeLocalHotspotState() {
    guard lanScope == .userApprovedHotspot else {
      localHotspotDetail = "local_hotspot_not_joined"
      return
    }
    switch localNetworkState {
    case "ready":
      localHotspotDetail = "local_hotspot_joined"
    case "failed":
      localHotspotDetail = "local_hotspot_binding_failed"
    default:
      localHotspotDetail = localHotspotDetail == "local_hotspot_joined"
        ? "local_hotspot_lost"
        : "local_hotspot_unavailable"
    }
  }

  private func capability(_ name: String, _ state: String, _ detail: String) -> [String: String] {
    ["name": name, "state": state, "detail": detail]
  }
}

private enum HaloAppleNativePairingOutcome: Equatable {
  case authenticated
  case failed
  case cancelled
}

private let haloAppleNativeStatusOK: Int32 = 0
private let haloAppleNativeStatusEmpty: Int32 = 1
private let haloAppleNativeStatusBackpressure: Int32 = 2

/// Pumps one native QUIC control stream directly into the Rust pairing
/// mailbox. The C ABI copies complete bounded frames synchronously and retains
/// no Swift pointers.
private func haloRunApplePairingBridge(
  sessionID: UInt64,
  channel: HaloAppleQuicControlChannel,
  direction: HaloApplePeerToPeerDirection,
  peerPresenceID: UUID?
) async -> HaloAppleNativePairingOutcome {
  let peerBytes = peerPresenceID.map {
    Data($0.uuidString.lowercased().utf8)
  } ?? Data()
  let binding = channel.channelBinding
  var channelID: UInt64 = 0
  let attachStatus = peerBytes.withUnsafeBytes { peerBuffer in
    binding.withUnsafeBytes { bindingBuffer in
      halo_apple_pairing_attach(
        sessionID,
        peerBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
        peerBuffer.count,
        direction == .outgoing ? 0 : 1,
        bindingBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
        bindingBuffer.count,
        &channelID
      )
    }
  }
  guard attachStatus == haloAppleNativeStatusOK, channelID != 0 else {
    return .failed
  }
  defer { _ = halo_apple_pairing_close(sessionID, channelID) }

  return await withTaskGroup(of: HaloAppleNativePairingOutcome.self) { group in
    group.addTask {
      await haloPumpApplePairingInbound(
        sessionID: sessionID,
        channelID: channelID,
        channel: channel
      )
    }
    group.addTask {
      await haloPumpApplePairingOutbound(
        sessionID: sessionID,
        channelID: channelID,
        channel: channel
      )
    }
    let outcome = await group.next() ?? .failed
    await channel.close()
    group.cancelAll()
    return outcome
  }
}

private func haloPumpApplePairingInbound(
  sessionID: UInt64,
  channelID: UInt64,
  channel: HaloAppleQuicControlChannel
) async -> HaloAppleNativePairingOutcome {
  do {
    while !Task.isCancelled {
      let frame = try await channel.receiveFrame()
      var status = frame.withUnsafeBytes { frameBuffer in
        halo_apple_pairing_submit(
          sessionID,
          channelID,
          frameBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
          frameBuffer.count
        )
      }
      while status == haloAppleNativeStatusBackpressure, !Task.isCancelled {
        try await Task.sleep(nanoseconds: 20_000_000)
        status = frame.withUnsafeBytes { frameBuffer in
          halo_apple_pairing_submit(
            sessionID,
            channelID,
            frameBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self),
            frameBuffer.count
          )
        }
      }
      guard status == haloAppleNativeStatusOK else {
        return haloApplePairingState(sessionID: sessionID, channelID: channelID)
      }
    }
    return .cancelled
  } catch is CancellationError {
    return .cancelled
  } catch {
    return await haloApplePairingStateAfterIOEnd(
      sessionID: sessionID,
      channelID: channelID
    )
  }
}

private func haloPumpApplePairingOutbound(
  sessionID: UInt64,
  channelID: UInt64,
  channel: HaloAppleQuicControlChannel
) async -> HaloAppleNativePairingOutcome {
  var frameBuffer = [UInt8](
    repeating: 0,
    count: HaloAppleQuicControlProtocol.maximumFrameLength
  )
  do {
    while !Task.isCancelled {
      let rawState = halo_apple_pairing_state(sessionID, channelID)
      if rawState == 1 { return .authenticated }
      if rawState == 2 || rawState < 0 { return .failed }

      var frameLength = 0
      let status = frameBuffer.withUnsafeMutableBufferPointer { buffer in
        halo_apple_pairing_drain(
          sessionID,
          channelID,
          buffer.baseAddress,
          buffer.count,
          &frameLength
        )
      }
      if status == haloAppleNativeStatusOK {
        try await channel.sendFrame(Data(frameBuffer.prefix(frameLength)))
      } else if status != haloAppleNativeStatusEmpty {
        return await haloApplePairingStateAfterIOEnd(
          sessionID: sessionID,
          channelID: channelID
        )
      }
      try await Task.sleep(nanoseconds: 20_000_000)
    }
    return .cancelled
  } catch is CancellationError {
    return .cancelled
  } catch {
    return await haloApplePairingStateAfterIOEnd(
      sessionID: sessionID,
      channelID: channelID
    )
  }
}

private func haloApplePairingState(
  sessionID: UInt64,
  channelID: UInt64
) -> HaloAppleNativePairingOutcome {
  switch halo_apple_pairing_state(sessionID, channelID) {
  case 1: .authenticated
  case 2: .failed
  default: .failed
  }
}

private func haloApplePairingStateAfterIOEnd(
  sessionID: UInt64,
  channelID: UInt64
) async -> HaloAppleNativePairingOutcome {
  for _ in 0 ..< 5 {
    let rawState = halo_apple_pairing_state(sessionID, channelID)
    if rawState == 1 { return .authenticated }
    if rawState == 2 || rawState < 0 { return .failed }
    do {
      try await Task.sleep(nanoseconds: 20_000_000)
    } catch {
      return .cancelled
    }
  }
  return .failed
}

private enum HaloIdentityStorageError: Error {
  case failed(OSStatus)
}
