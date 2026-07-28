@preconcurrency import CoreBluetooth
import Foundation

public enum HaloBleUUID {
    public static let publicLocalName = "Halo"
    public static let service = CBUUID(string: "B6882C7F-D426-4CB6-9012-D40BDE5E2000")
    public static let presence = CBUUID(string: "8C2E5E61-4C6A-4C64-B804-1301A15251A0")
    public static let wakeLan = CBUUID(string: "4FE6E851-DBC1-4A86-8E49-FCF1EABC1C82")
    public static let endpointHints = CBUUID(string: "4672307B-CAEA-4E1A-8823-0BCEA898EC83")
}

/// Returns the remaining value for an ATT Read/Read Blob request.
///
/// Android commonly continues a characteristic read with a non-zero offset
/// when the 58-byte Presence value exceeds the negotiated ATT payload.
func haloBleReadSlice(_ value: Data, offset: Int) -> Data? {
    guard offset >= 0, offset <= value.count else { return nil }
    return value.subdata(in: offset ..< value.count)
}

public enum HaloBleConfigurationError: Error, Equatable {
    case invalidPresenceLength(Int)
    case invalidConnectionLimit(Int)
    case invalidInterval(String)
}

func haloBleBoundedBackoff(
    base: TimeInterval,
    maximum: TimeInterval,
    attempt: Int
) -> TimeInterval {
    let exponent = min(max(attempt - 1, 0), 30)
    return min(base * pow(2, Double(exponent)), maximum)
}

public struct HaloBleConfiguration: Sendable {
    public static let presenceLength = 58

    public let presence: Data
    public let maximumConcurrentGattConnections: Int
    public let refreshInterval: Duration
    public let connectionTimeout: TimeInterval
    public let peerRetryBase: TimeInterval
    public let peerRetryMaximum: TimeInterval
    public let advertisingRetryBase: TimeInterval
    public let advertisingRetryMaximum: TimeInterval

    public init(
        presence: Data,
        maximumConcurrentGattConnections: Int = 2,
        refreshInterval: Duration = .seconds(10),
        connectionTimeout: TimeInterval = 8,
        peerRetryBase: TimeInterval = 2,
        peerRetryMaximum: TimeInterval = 60,
        advertisingRetryBase: TimeInterval = 2,
        advertisingRetryMaximum: TimeInterval = 30
    ) throws {
        guard presence.count == Self.presenceLength else {
            throw HaloBleConfigurationError.invalidPresenceLength(presence.count)
        }
        guard (1 ... 8).contains(maximumConcurrentGattConnections) else {
            throw HaloBleConfigurationError.invalidConnectionLimit(
                maximumConcurrentGattConnections
            )
        }
        guard (1 ... 60).contains(connectionTimeout) else {
            throw HaloBleConfigurationError.invalidInterval("connectionTimeout")
        }
        guard peerRetryBase >= 1, peerRetryMaximum >= peerRetryBase else {
            throw HaloBleConfigurationError.invalidInterval("peerRetry")
        }
        guard advertisingRetryBase >= 1,
              advertisingRetryMaximum >= advertisingRetryBase
        else {
            throw HaloBleConfigurationError.invalidInterval("advertisingRetry")
        }
        self.presence = presence
        self.maximumConcurrentGattConnections = maximumConcurrentGattConnections
        self.refreshInterval = refreshInterval
        self.connectionTimeout = connectionTimeout
        self.peerRetryBase = peerRetryBase
        self.peerRetryMaximum = peerRetryMaximum
        self.advertisingRetryBase = advertisingRetryBase
        self.advertisingRetryMaximum = advertisingRetryMaximum
    }
}

public enum HaloBleState: Sendable, Equatable {
    case starting
    case ready
    case poweredOff
    case unauthorized
    case unsupported
    case resetting
    case stopped
}

public enum HaloBleDiagnostic: Sendable, Equatable {
    case scanFailed(String)
    case connectionFailed(String)
    case serviceDiscoveryFailed(String)
    case characteristicDiscoveryFailed(String)
    case presenceReadFailed(String)
    case invalidPresenceLength(Int)
    case advertisingFailed(String)
}

public enum HaloBleEvent: Sendable, Equatable {
    case state(HaloBleState)
    case presence(peripheralHandle: UUID, descriptor: Data, rssi: Int)
    case diagnostic(HaloBleDiagnostic)
}

/// Foreground BLE rendezvous shared by the iOS and macOS applications.
///
/// The provider intentionally emits raw Presence bytes. The Rust codec remains
/// the only parser and performs all protocol validation before aggregation.
public final class HaloBleProvider: NSObject, @unchecked Sendable {
    public typealias EventHandler = @Sendable (HaloBleEvent) -> Void
    public typealias WakeLanHandler = @Sendable (Data) -> UInt8

    private let queue: DispatchQueue
    private let queueKey = DispatchSpecificKey<UInt8>()
    private let eventHandler: EventHandler
    private let wakeLanHandler: WakeLanHandler
    private let configuration: HaloBleConfiguration
    private var presence: Data

    private var central: CBCentralManager?
    private var peripheralManager: CBPeripheralManager?
    private var presenceCharacteristic: CBMutableCharacteristic?
    private var wakeCharacteristic: CBMutableCharacteristic?
    private var serviceInstalled = false
    private var started = false

    private var peripherals: [UUID: CBPeripheral] = [:]
    private var pending: [UUID] = []
    private var connecting: Set<UUID> = []
    private var lastReadAt: [UUID: ContinuousClock.Instant] = [:]
    private var latestRssi: [UUID: Int] = [:]
    private var connectionTimeouts: [UUID: DispatchWorkItem] = [:]
    private var peerRetryAttempts: [UUID: Int] = [:]
    private var peerRetryNotBefore: [UUID: ContinuousClock.Instant] = [:]
    private var advertisingRetryAttempt = 0
    private var advertisingRetry: DispatchWorkItem?

    public init(
        configuration: HaloBleConfiguration,
        eventHandler: @escaping EventHandler,
        wakeLanHandler: @escaping WakeLanHandler
    ) {
        self.configuration = configuration
        presence = configuration.presence
        self.eventHandler = eventHandler
        self.wakeLanHandler = wakeLanHandler
        queue = DispatchQueue(label: "org.halo.discovery.ble", qos: .userInitiated)
        super.init()
        queue.setSpecific(key: queueKey, value: 1)
    }

    public func start() {
        queue.async { [weak self] in
            guard let self, !started else { return }
            started = true
            eventHandler(.state(.starting))
            central = CBCentralManager(delegate: self, queue: queue)
            peripheralManager = CBPeripheralManager(delegate: self, queue: queue)
        }
    }

    public func stop() {
        // Capture strongly: the bridge may release its last provider reference
        // immediately after this call, but cleanup still has to run.
        queue.async { self.stopInternal() }
    }

    /// Synchronously unregisters CoreBluetooth resources before a replacement
    /// provider is created. This prevents old and new scans from overlapping.
    public func stopAndWait() {
        if DispatchQueue.getSpecific(key: queueKey) != nil {
            stopInternal()
        } else {
            queue.sync { self.stopInternal() }
        }
    }

    private func stopInternal() {
        let wasStarted = started
        started = false
        central?.stopScan()
        for peripheral in peripherals.values {
            central?.cancelPeripheralConnection(peripheral)
        }
        peripheralManager?.stopAdvertising()
        peripheralManager?.removeAllServices()
        connectionTimeouts.values.forEach { $0.cancel() }
        advertisingRetry?.cancel()
        peripherals.removeAll(keepingCapacity: false)
        pending.removeAll(keepingCapacity: false)
        connecting.removeAll(keepingCapacity: false)
        lastReadAt.removeAll(keepingCapacity: false)
        latestRssi.removeAll(keepingCapacity: false)
        connectionTimeouts.removeAll(keepingCapacity: false)
        peerRetryAttempts.removeAll(keepingCapacity: false)
        peerRetryNotBefore.removeAll(keepingCapacity: false)
        advertisingRetry = nil
        advertisingRetryAttempt = 0
        presenceCharacteristic = nil
        wakeCharacteristic = nil
        serviceInstalled = false
        central = nil
        peripheralManager = nil
        if wasStarted { eventHandler(.state(.stopped)) }
    }

    public func updatePresence(_ data: Data) throws {
        guard data.count == HaloBleConfiguration.presenceLength else {
            throw HaloBleConfigurationError.invalidPresenceLength(data.count)
        }
        queue.async { [weak self] in
            guard let self else { return }
            presence = data
            guard let presenceCharacteristic else { return }
            presenceCharacteristic.value = data
            _ = peripheralManager?.updateValue(
                data,
                for: presenceCharacteristic,
                onSubscribedCentrals: nil
            )
        }
    }

    private func beginScanningIfPossible() {
        guard started, central?.state == .poweredOn else { return }
        central?.scanForPeripherals(
            withServices: [HaloBleUUID.service],
            options: [CBCentralManagerScanOptionAllowDuplicatesKey: true]
        )
        emitReadyIfPossible()
    }

    private func installGattServiceIfPossible() {
        guard started, peripheralManager?.state == .poweredOn, !serviceInstalled else { return }
        let presence = CBMutableCharacteristic(
            type: HaloBleUUID.presence,
            properties: [.read, .notify],
            value: nil,
            permissions: [.readable]
        )
        let wake = CBMutableCharacteristic(
            type: HaloBleUUID.wakeLan,
            properties: [.write, .notify],
            value: nil,
            permissions: [.writeable]
        )
        let service = CBMutableService(type: HaloBleUUID.service, primary: true)
        service.characteristics = [presence, wake]
        presenceCharacteristic = presence
        wakeCharacteristic = wake
        serviceInstalled = true
        peripheralManager?.add(service)
    }

    private func emitReadyIfPossible() {
        if started, central?.state == .poweredOn, peripheralManager?.state == .poweredOn,
           peripheralManager?.isAdvertising == true
        {
            eventHandler(.state(.ready))
        }
    }

    private func queueGattRead(for peripheral: CBPeripheral) {
        let id = peripheral.identifier
        if connecting.contains(id) || pending.contains(id) { return }
        if let retryAt = peerRetryNotBefore[id], ContinuousClock.now < retryAt { return }
        if let lastRead = lastReadAt[id],
           ContinuousClock.now - lastRead < configuration.refreshInterval
        {
            return
        }
        pending.append(id)
        startPendingConnections()
    }

    private func startPendingConnections() {
        guard let central, central.state == .poweredOn else { return }
        while connecting.count < configuration.maximumConcurrentGattConnections,
              !pending.isEmpty
        {
            let id = pending.removeFirst()
            guard let peripheral = peripherals[id] else { continue }
            connecting.insert(id)
            peripheral.delegate = self
            central.connect(peripheral, options: nil)
            let timeout = DispatchWorkItem { [weak self] in
                guard let self, connecting.contains(id),
                      let current = peripherals[id]
                else { return }
                eventHandler(.diagnostic(.connectionFailed("connection timed out")))
                finish(current)
            }
            connectionTimeouts[id] = timeout
            queue.asyncAfter(deadline: .now() + configuration.connectionTimeout, execute: timeout)
        }
    }

    private func finish(_ peripheral: CBPeripheral, succeeded: Bool = false) {
        let id = peripheral.identifier
        connectionTimeouts.removeValue(forKey: id)?.cancel()
        connecting.remove(id)
        if succeeded {
            peerRetryAttempts.removeValue(forKey: id)
            peerRetryNotBefore.removeValue(forKey: id)
        } else {
            let attempt = min((peerRetryAttempts[id] ?? 0) + 1, 8)
            peerRetryAttempts[id] = attempt
            let delay = haloBleBoundedBackoff(
                base: configuration.peerRetryBase,
                maximum: configuration.peerRetryMaximum,
                attempt: attempt
            )
            peerRetryNotBefore[id] = ContinuousClock.now.advanced(by: .seconds(delay))
        }
        central?.cancelPeripheralConnection(peripheral)
        startPendingConnections()
    }

    private func resetCentralWork() {
        connectionTimeouts.values.forEach { $0.cancel() }
        connectionTimeouts.removeAll(keepingCapacity: false)
        pending.removeAll(keepingCapacity: false)
        connecting.removeAll(keepingCapacity: false)
    }

    private func resetPeripheralWork() {
        advertisingRetry?.cancel()
        advertisingRetry = nil
        advertisingRetryAttempt = 0
        serviceInstalled = false
        presenceCharacteristic = nil
        wakeCharacteristic = nil
    }

    private func scheduleAdvertisingRetry(reinstallService: Bool) {
        guard started, peripheralManager?.state == .poweredOn else { return }
        advertisingRetry?.cancel()
        advertisingRetryAttempt = min(advertisingRetryAttempt + 1, 8)
        let delay = haloBleBoundedBackoff(
            base: configuration.advertisingRetryBase,
            maximum: configuration.advertisingRetryMaximum,
            attempt: advertisingRetryAttempt
        )
        let retry = DispatchWorkItem { [weak self] in
            guard let self, started, let manager = peripheralManager,
                  manager.state == .poweredOn
            else { return }
            if reinstallService {
                installGattServiceIfPossible()
            } else {
                manager.startAdvertising([
                    CBAdvertisementDataLocalNameKey: HaloBleUUID.publicLocalName,
                    CBAdvertisementDataServiceUUIDsKey: [HaloBleUUID.service],
                ])
            }
        }
        advertisingRetry = retry
        queue.asyncAfter(deadline: .now() + delay, execute: retry)
    }

    private func mapState(_ state: CBManagerState) -> HaloBleState {
        switch state {
        case .poweredOn: .starting
        case .poweredOff: .poweredOff
        case .unauthorized: .unauthorized
        case .unsupported: .unsupported
        case .resetting: .resetting
        case .unknown: .starting
        @unknown default: .unsupported
        }
    }
}

extension HaloBleProvider: CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        if central.state == .poweredOn {
            beginScanningIfPossible()
        } else {
            resetCentralWork()
            eventHandler(.state(mapState(central.state)))
        }
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData _: [String: Any],
        rssi RSSI: NSNumber
    ) {
        guard started else { return }
        peripherals[peripheral.identifier] = peripheral
        latestRssi[peripheral.identifier] = RSSI.intValue
        queueGattRead(for: peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.discoverServices([HaloBleUUID.service])
    }

    public func centralManager(
        _ central: CBCentralManager,
        didFailToConnect peripheral: CBPeripheral,
        error: (any Error)?
    ) {
        eventHandler(.diagnostic(.connectionFailed(error?.localizedDescription ?? "unknown")))
        finish(peripheral)
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral peripheral: CBPeripheral,
        error: (any Error)?
    ) {
        let id = peripheral.identifier
        let wasConnecting = connecting.remove(id) != nil
        connectionTimeouts.removeValue(forKey: id)?.cancel()
        if let error, wasConnecting {
            eventHandler(.diagnostic(.connectionFailed(error.localizedDescription)))
            let attempt = min((peerRetryAttempts[id] ?? 0) + 1, 8)
            peerRetryAttempts[id] = attempt
            let delay = haloBleBoundedBackoff(
                base: configuration.peerRetryBase,
                maximum: configuration.peerRetryMaximum,
                attempt: attempt
            )
            peerRetryNotBefore[id] = ContinuousClock.now.advanced(by: .seconds(delay))
        }
        startPendingConnections()
    }
}

extension HaloBleProvider: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: (any Error)?) {
        if let error {
            eventHandler(.diagnostic(.serviceDiscoveryFailed(error.localizedDescription)))
            finish(peripheral)
            return
        }
        guard let service = peripheral.services?.first(where: { $0.uuid == HaloBleUUID.service })
        else {
            eventHandler(.diagnostic(.serviceDiscoveryFailed("Halo service missing")))
            finish(peripheral)
            return
        }
        peripheral.discoverCharacteristics([HaloBleUUID.presence], for: service)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: (any Error)?
    ) {
        if let error {
            eventHandler(.diagnostic(.characteristicDiscoveryFailed(error.localizedDescription)))
            finish(peripheral)
            return
        }
        guard let characteristic = service.characteristics?.first(where: {
            $0.uuid == HaloBleUUID.presence
        }) else {
            eventHandler(.diagnostic(.characteristicDiscoveryFailed("Presence missing")))
            finish(peripheral)
            return
        }
        peripheral.readValue(for: characteristic)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: (any Error)?
    ) {
        if let error {
            eventHandler(.diagnostic(.presenceReadFailed(error.localizedDescription)))
            finish(peripheral)
            return
        }
        guard characteristic.uuid == HaloBleUUID.presence, let data = characteristic.value else {
            finish(peripheral)
            return
        }
        guard data.count == HaloBleConfiguration.presenceLength else {
            eventHandler(.diagnostic(.invalidPresenceLength(data.count)))
            finish(peripheral)
            return
        }
        lastReadAt[peripheral.identifier] = .now
        eventHandler(
            .presence(
                peripheralHandle: peripheral.identifier,
                descriptor: data,
                rssi: latestRssi[peripheral.identifier] ?? 0
            )
        )
        finish(peripheral, succeeded: true)
    }
}

extension HaloBleProvider: CBPeripheralManagerDelegate {
    public func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        if peripheral.state == .poweredOn {
            installGattServiceIfPossible()
        } else {
            resetPeripheralWork()
            eventHandler(.state(mapState(peripheral.state)))
        }
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didAdd service: CBService,
        error: (any Error)?
    ) {
        if let error {
            serviceInstalled = false
            eventHandler(.diagnostic(.advertisingFailed(error.localizedDescription)))
            scheduleAdvertisingRetry(reinstallService: true)
            return
        }
        advertisingRetryAttempt = 0
        peripheral.startAdvertising([
            CBAdvertisementDataLocalNameKey: HaloBleUUID.publicLocalName,
            CBAdvertisementDataServiceUUIDsKey: [HaloBleUUID.service],
        ])
    }

    public func peripheralManagerDidStartAdvertising(
        _ peripheral: CBPeripheralManager,
        error: (any Error)?
    ) {
        if let error {
            eventHandler(.diagnostic(.advertisingFailed(error.localizedDescription)))
            scheduleAdvertisingRetry(reinstallService: false)
        } else {
            advertisingRetry?.cancel()
            advertisingRetry = nil
            advertisingRetryAttempt = 0
            emitReadyIfPossible()
        }
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didReceiveRead request: CBATTRequest
    ) {
        guard request.characteristic.uuid == HaloBleUUID.presence else {
            peripheral.respond(to: request, withResult: .requestNotSupported)
            return
        }
        guard let value = haloBleReadSlice(presence, offset: request.offset) else {
            peripheral.respond(to: request, withResult: .invalidOffset)
            return
        }
        request.value = value
        peripheral.respond(to: request, withResult: .success)
    }

    public func peripheralManager(
        _ peripheral: CBPeripheralManager,
        didReceiveWrite requests: [CBATTRequest]
    ) {
        for request in requests {
            guard request.characteristic.uuid == HaloBleUUID.wakeLan,
                  request.offset == 0,
                  let nonce = request.value,
                  nonce.count == 8
            else {
                peripheral.respond(to: request, withResult: .invalidAttributeValueLength)
                continue
            }
            let status = wakeLanHandler(nonce)
            var response = nonce
            response.append(status)
            peripheral.respond(to: request, withResult: .success)
            if let wakeCharacteristic {
                _ = peripheral.updateValue(
                    response,
                    for: wakeCharacteristic,
                    onSubscribedCentrals: [request.central]
                )
            }
        }
    }
}
