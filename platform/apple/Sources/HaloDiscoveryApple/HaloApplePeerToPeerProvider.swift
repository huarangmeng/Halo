@preconcurrency import Network
import Foundation
import Security

public enum HaloApplePeerToPeerProtocol {
    public static let serviceType = "_halo._udp"
    public static let alpn = "halo-pairing/1"
}

public enum HaloApplePeerToPeerConfigurationError: Error, Equatable {
    case invalidInstanceName
    case invalidCandidateLimit(Int)
    case invalidConnectionTimeout
    case unsafeNetworkParameters
}

/// Configuration for Apple's infrastructure-less Network.framework path.
/// `instanceName` must be an opaque, rotating rendezvous identifier. It must
/// not contain a device name or a stable peer identity.
public struct HaloApplePeerToPeerConfiguration: Sendable {
    public let instanceName: String
    public let maximumCandidates: Int
    public let connectionTimeout: TimeInterval

    public init(
        instanceName: String,
        maximumCandidates: Int = 64,
        connectionTimeout: TimeInterval = 10
    ) throws {
        let nameLength = instanceName.lengthOfBytes(using: .utf8)
        guard (1 ... 63).contains(nameLength) else {
            throw HaloApplePeerToPeerConfigurationError.invalidInstanceName
        }
        guard (1 ... 256).contains(maximumCandidates) else {
            throw HaloApplePeerToPeerConfigurationError.invalidCandidateLimit(maximumCandidates)
        }
        guard (1 ... 60).contains(connectionTimeout) else {
            throw HaloApplePeerToPeerConfigurationError.invalidConnectionTimeout
        }
        self.instanceName = instanceName
        self.maximumCandidates = maximumCandidates
        self.connectionTimeout = connectionTimeout
    }
}

/// Centralizes the non-cellular path policy for every Apple P2P connection.
public enum HaloApplePeerToPeerNetworkPolicy {
    /// Creates QUIC parameters after the caller configures TLS identity and
    /// peer-certificate verification. Halo pairing must subsequently bind to
    /// the QUIC TLS exporter before the returned connection is trusted.
    public static func makeQuicParameters(
        configureSecurity: (sec_protocol_options_t) throws -> Void
    ) rethrows -> NWParameters {
        let options = NWProtocolQUIC.Options(alpn: [HaloApplePeerToPeerProtocol.alpn])
        options.direction = .bidirectional
        options.isDatagram = false
        options.idleTimeout = 30_000
        options.initialMaxStreamsBidirectional = 4
        options.initialMaxStreamsUnidirectional = 0
        try configureSecurity(options.securityProtocolOptions)
        let parameters = NWParameters(quic: options)
        parameters.includePeerToPeer = true
        parameters.requiredInterfaceType = .wifi
        parameters.prohibitedInterfaceTypes = [.cellular]
        return parameters
    }

    public static func parametersAreEligible(_ parameters: NWParameters) -> Bool {
        parameters.includePeerToPeer
            && parameters.requiredInterfaceType == .wifi
            && parameters.prohibitedInterfaceTypes?.contains(.cellular) == true
            && parameters.defaultProtocolStack.transportProtocol is NWProtocolQUIC.Options
    }

    public static func pathIsEligible(_ path: NWPath) -> Bool {
        path.status == .satisfied
            && path.usesInterfaceType(.wifi)
            && !path.usesInterfaceType(.cellular)
    }
}

public enum HaloApplePeerToPeerState: Sendable, Equatable {
    case starting
    case ready
    case temporarilyUnavailable
    case failed
    case stopped
}

public enum HaloApplePeerToPeerDirection: String, Sendable, Equatable {
    case incoming
    case outgoing
}

public enum HaloApplePeerToPeerFailure: String, Sendable, Equatable {
    case invalidParameters
    case browser
    case listener
    case connection
    case ineligiblePath
    case timedOut
    case candidateLimit
}

/// Events carry process-local opaque handles plus the rotating presence UUID
/// used to correlate this bearer with shared discovery. Bonjour endpoints,
/// interface names, addresses, and arbitrary service names never cross into
/// Flutter or diagnostics.
public enum HaloApplePeerToPeerEvent: Sendable, Equatable {
    case state(HaloApplePeerToPeerState)
    case candidateFound(handle: UUID, peerPresenceID: UUID)
    case candidateLost(handle: UUID, peerPresenceID: UUID)
    case linkReady(
        handle: UUID,
        direction: HaloApplePeerToPeerDirection,
        peerPresenceID: UUID?
    )
    case linkFailed(handle: UUID, failure: HaloApplePeerToPeerFailure)
    case dataStreamReady(
        handle: UUID,
        sessionHandle: UUID,
        direction: HaloApplePeerToPeerDirection
    )
    case dataStreamFailed(handle: UUID, failure: HaloApplePeerToPeerFailure)
    case diagnostic(HaloApplePeerToPeerFailure)
}

/// Apple-only P2P bearer based on Bonjour discovery plus Network.framework
/// QUIC with `includePeerToPeer`. The provider establishes an unauthenticated
/// bearer; the shared Halo handshake is still mandatory before use.
public final class HaloApplePeerToPeerProvider: @unchecked Sendable {
    public typealias EventHandler = @Sendable (HaloApplePeerToPeerEvent) -> Void
    public typealias ParametersFactory = @Sendable () throws -> NWParameters

    private let configuration: HaloApplePeerToPeerConfiguration
    private let parametersFactory: ParametersFactory
    private let eventHandler: EventHandler
    private let queue = DispatchQueue(label: "org.halo.transport.apple-p2p", qos: .userInitiated)
    private let queueKey = DispatchSpecificKey<UInt8>()

    private var browser: NWBrowser?
    private var listener: NWListener?
    private var started = false
    private var browserReady = false
    private var listenerReady = false
    private var lastState: HaloApplePeerToPeerState?
    private var handlesByEndpoint: [NWEndpoint: UUID] = [:]
    private var endpointsByHandle: [UUID: NWEndpoint] = [:]
    private var presenceByHandle: [UUID: UUID] = [:]
    private var pendingResolvers: [UUID: HaloBonjourServiceResolver] = [:]
    private var tunnels: [UUID: Tunnel] = [:]
    private var connections: [UUID: NWConnection] = [:]
    private var dataConnections: [UUID: NWConnection] = [:]
    private var dataStreamSessions: [UUID: UUID] = [:]
    private var connectionTimeouts: [UUID: DispatchWorkItem] = [:]

    private struct Tunnel {
        let group: NWConnectionGroup
        let direction: HaloApplePeerToPeerDirection
        let peerPresenceID: UUID?
        var authenticated = false
        var channelBinding: Data? = nil
    }

    public init(
        configuration: HaloApplePeerToPeerConfiguration,
        parametersFactory: @escaping ParametersFactory,
        eventHandler: @escaping EventHandler
    ) {
        self.configuration = configuration
        self.parametersFactory = parametersFactory
        self.eventHandler = eventHandler
        queue.setSpecific(key: queueKey, value: 1)
    }

    public func start() {
        queue.async { [weak self] in self?.startInternal() }
    }

    public func stop() {
        queue.async { self.stopInternal() }
    }

    public func stopAndWait() {
        if DispatchQueue.getSpecific(key: queueKey) != nil {
            stopInternal()
        } else {
            queue.sync { stopInternal() }
        }
    }

    /// Starts a connection to a provider-owned candidate and returns an opaque
    /// link handle. The ready connection remains native and is never exposed to
    /// Dart; the bounded bridge exposes only frames and exporter bytes needed
    /// for Rust to perform Halo pairing.
    public func connect(candidateHandle: UUID) -> UUID? {
        onQueue {
            guard started,
                  let endpoint = endpointsByHandle[candidateHandle],
                  let peerPresenceID = presenceByHandle[candidateHandle]
            else {
                return nil
            }
            let handle = UUID()
            do {
                let resolver = try HaloBonjourServiceResolver(
                    endpoint: endpoint,
                    queue: queue,
                    completion: { [weak self] result in
                        self?.completeResolution(
                            result,
                            handle: handle,
                            peerPresenceID: peerPresenceID
                        )
                    }
                )
                pendingResolvers[handle] = resolver
                installTimeout(handle: handle)
                try resolver.start()
                return handle
            } catch {
                pendingResolvers.removeValue(forKey: handle)
                connectionTimeouts.removeValue(forKey: handle)?.cancel()
                eventHandler(.linkFailed(handle: handle, failure: .invalidParameters))
                return nil
            }
        }
    }

    /// Transfers a ready native connection into a bounded Halo control stream.
    /// Flutter receives only the corresponding opaque UUID and never owns the
    /// `NWConnection`.
    public func takeControlChannel(
        handle: UUID
    ) throws -> HaloAppleQuicControlChannel? {
        try onQueue {
            connectionTimeouts.removeValue(forKey: handle)?.cancel()
            guard let connection = connections.removeValue(forKey: handle) else { return nil }
            do {
                return try HaloAppleQuicControlChannel(connection: connection)
            } catch {
                connection.cancel()
                throw error
            }
        }
    }

    public func cancelConnection(handle: UUID) {
        onQueue {
            cancelTunnel(handle: handle)
        }
    }

    /// Ends the pairing stream while retaining an authenticated QUIC tunnel
    /// for later transfer streams. A failed pairing tears down the tunnel.
    public func finishPairing(
        handle: UUID,
        authenticated: Bool,
        channelBinding: Data? = nil
    ) {
        onQueue {
            connections.removeValue(forKey: handle)?.cancel()
            if authenticated,
               channelBinding?.count == HaloAppleQuicControlProtocol.exporterLength,
               var tunnel = tunnels[handle]
            {
                tunnel.authenticated = true
                tunnel.channelBinding = channelBinding
                tunnels[handle] = tunnel
            } else {
                cancelTunnel(handle: handle)
            }
        }
    }

    /// Opens a new bidirectional file-data stream on an authenticated tunnel.
    /// The returned handle is process-local and the `NWConnection` remains
    /// native until `takeDataStream` consumes it.
    public func openDataStream(sessionHandle: UUID) -> UUID? {
        onQueue {
            guard let tunnel = tunnels[sessionHandle],
                  tunnel.authenticated,
                  tunnel.channelBinding != nil,
                  let connection = NWConnection(from: tunnel.group)
            else {
                return nil
            }
            let handle = UUID()
            registerDataStream(
                connection,
                handle: handle,
                sessionHandle: sessionHandle,
                direction: .outgoing
            )
            connection.start(queue: queue)
            return handle
        }
    }

    public func takeDataStream(handle: UUID) throws -> HaloAppleQuicDataStream? {
        try onQueue {
            guard let connection = dataConnections.removeValue(forKey: handle),
                  let sessionHandle = dataStreamSessions.removeValue(forKey: handle),
                  let binding = tunnels[sessionHandle]?.channelBinding
            else {
                return nil
            }
            do {
                return try HaloAppleQuicDataStream(
                    connection: connection,
                    expectedChannelBinding: binding
                )
            } catch {
                connection.cancel()
                throw error
            }
        }
    }

    public func cancelDataStream(handle: UUID) {
        onQueue {
            dataStreamSessions.removeValue(forKey: handle)
            dataConnections.removeValue(forKey: handle)?.cancel()
        }
    }

    private func startInternal() {
        guard !started else { return }
        emitState(.starting)
        do {
            let browserParameters = try safeParameters()
            let listenerParameters = try safeParameters()
            let browser = NWBrowser(
                for: .bonjour(type: HaloApplePeerToPeerProtocol.serviceType, domain: nil),
                using: browserParameters
            )
            let listener = try NWListener(using: listenerParameters)
            listener.service = NWListener.Service(
                name: configuration.instanceName,
                type: HaloApplePeerToPeerProtocol.serviceType
            )
            installHandlers(browser: browser, listener: listener)
            self.browser = browser
            self.listener = listener
            started = true
            browser.start(queue: queue)
            listener.start(queue: queue)
        } catch {
            emitState(.failed)
            eventHandler(.diagnostic(.invalidParameters))
            stopResources(emitStopped: false)
        }
    }

    private func installHandlers(browser: NWBrowser, listener: NWListener) {
        browser.stateUpdateHandler = { [weak self] state in
            self?.handleBrowserState(state)
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            self?.replaceCandidates(with: results)
        }
        listener.stateUpdateHandler = { [weak self] state in
            self?.handleListenerState(state)
        }
        listener.newConnectionGroupHandler = { [weak self] group in
            guard let self else {
                group.cancel()
                return
            }
            let handle = UUID()
            register(
                group,
                handle: handle,
                direction: .incoming,
                peerPresenceID: nil
            )
            installTimeout(handle: handle)
            group.start(queue: queue)
        }
    }

    private func handleBrowserState(_ state: NWBrowser.State) {
        guard started else { return }
        switch state {
        case .ready:
            browserReady = true
            emitCombinedState()
        case .waiting:
            browserReady = false
            emitState(.temporarilyUnavailable)
        case .failed:
            browserReady = false
            emitState(.failed)
            eventHandler(.diagnostic(.browser))
        case .cancelled:
            browserReady = false
        case .setup:
            break
        @unknown default:
            browserReady = false
            emitState(.temporarilyUnavailable)
        }
    }

    private func handleListenerState(_ state: NWListener.State) {
        guard started else { return }
        switch state {
        case .ready:
            listenerReady = true
            emitCombinedState()
        case .waiting:
            listenerReady = false
            emitState(.temporarilyUnavailable)
        case .failed:
            listenerReady = false
            emitState(.failed)
            eventHandler(.diagnostic(.listener))
        case .cancelled:
            listenerReady = false
        case .setup:
            break
        @unknown default:
            listenerReady = false
            emitState(.temporarilyUnavailable)
        }
    }

    private func replaceCandidates(with results: Set<NWBrowser.Result>) {
        guard started else { return }
        let endpoints = Set(results.compactMap { result -> NWEndpoint? in
            guard let peerPresenceID = Self.presenceID(from: result.endpoint),
                  peerPresenceID.uuidString.lowercased() != configuration.instanceName.lowercased()
            else {
                return nil
            }
            return result.endpoint
        })

        for endpoint in handlesByEndpoint.keys where !endpoints.contains(endpoint) {
            if let handle = handlesByEndpoint.removeValue(forKey: endpoint) {
                endpointsByHandle.removeValue(forKey: handle)
                if let peerPresenceID = presenceByHandle.removeValue(forKey: handle) {
                    eventHandler(.candidateLost(handle: handle, peerPresenceID: peerPresenceID))
                }
            }
        }
        for endpoint in endpoints where handlesByEndpoint[endpoint] == nil {
            guard handlesByEndpoint.count < configuration.maximumCandidates else {
                eventHandler(.diagnostic(.candidateLimit))
                break
            }
            let handle = UUID()
            guard let peerPresenceID = Self.presenceID(from: endpoint) else { continue }
            handlesByEndpoint[endpoint] = handle
            endpointsByHandle[handle] = endpoint
            presenceByHandle[handle] = peerPresenceID
            eventHandler(.candidateFound(handle: handle, peerPresenceID: peerPresenceID))
        }
    }

    private func completeResolution(
        _ result: Result<HaloResolvedBonjourEndpoint, Error>,
        handle: UUID,
        peerPresenceID: UUID
    ) {
        guard started, pendingResolvers.removeValue(forKey: handle) != nil else { return }
        do {
            let resolved = try result.get()
            guard let endpoint = resolved.networkEndpoint else {
                throw HaloBonjourServiceResolverError.invalidResult
            }
            let descriptor = NWMultiplexGroup(to: endpoint)
            let group = NWConnectionGroup(with: descriptor, using: try safeParameters())
            register(
                group,
                handle: handle,
                direction: .outgoing,
                peerPresenceID: peerPresenceID
            )
            group.start(queue: queue)
        } catch {
            connectionTimeouts.removeValue(forKey: handle)?.cancel()
            eventHandler(.linkFailed(handle: handle, failure: .connection))
        }
    }

    private func register(
        _ group: NWConnectionGroup,
        handle: UUID,
        direction: HaloApplePeerToPeerDirection,
        peerPresenceID: UUID?
    ) {
        guard HaloApplePeerToPeerNetworkPolicy.parametersAreEligible(group.parameters) else {
            group.cancel()
            eventHandler(.linkFailed(handle: handle, failure: .invalidParameters))
            return
        }
        tunnels[handle] = Tunnel(
            group: group,
            direction: direction,
            peerPresenceID: peerPresenceID
        )
        group.newConnectionHandler = { [weak self, weak group] connection in
            guard let self, let group, tunnels[handle]?.group === group else {
                connection.cancel()
                return
            }
            if tunnels[handle]?.authenticated == true {
                let dataHandle = UUID()
                registerDataStream(
                    connection,
                    handle: dataHandle,
                    sessionHandle: handle,
                    direction: .incoming
                )
                connection.start(queue: queue)
            } else if connections[handle] == nil {
                register(
                    connection,
                    handle: handle,
                    direction: direction,
                    peerPresenceID: peerPresenceID
                )
                connection.start(queue: queue)
            } else {
                connection.cancel()
            }
        }
        group.stateUpdateHandler = { [weak self, weak group] state in
            guard let self, let group, tunnels[handle]?.group === group else { return }
            switch state {
            case .ready where direction == .outgoing:
                guard connections[handle] == nil,
                      let connection = NWConnection(from: group)
                else {
                    cancelTunnel(handle: handle)
                    eventHandler(.linkFailed(handle: handle, failure: .connection))
                    return
                }
                register(
                    connection,
                    handle: handle,
                    direction: direction,
                    peerPresenceID: peerPresenceID
                )
                connection.start(queue: queue)
            case .failed:
                cancelTunnel(handle: handle)
                eventHandler(.linkFailed(handle: handle, failure: .connection))
            case .cancelled:
                tunnels.removeValue(forKey: handle)
            case .setup, .waiting, .ready:
                break
            @unknown default:
                break
            }
        }
    }

    private func register(
        _ connection: NWConnection,
        handle: UUID,
        direction: HaloApplePeerToPeerDirection,
        peerPresenceID: UUID?
    ) {
        connections[handle] = connection
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection, connections[handle] === connection else { return }
            switch state {
            case .ready:
                guard let path = connection.currentPath,
                      HaloApplePeerToPeerNetworkPolicy.pathIsEligible(path)
                else {
                    connectionTimeouts.removeValue(forKey: handle)?.cancel()
                    cancelTunnel(handle: handle)
                    eventHandler(.linkFailed(handle: handle, failure: .ineligiblePath))
                    return
                }
                connectionTimeouts.removeValue(forKey: handle)?.cancel()
                eventHandler(.linkReady(
                    handle: handle,
                    direction: direction,
                    peerPresenceID: peerPresenceID
                ))
            case .failed:
                connectionTimeouts.removeValue(forKey: handle)?.cancel()
                connections.removeValue(forKey: handle)
                cancelTunnel(handle: handle)
                eventHandler(.linkFailed(handle: handle, failure: .connection))
            case .cancelled:
                connectionTimeouts.removeValue(forKey: handle)?.cancel()
                connections.removeValue(forKey: handle)
            case .setup, .preparing, .waiting:
                break
            @unknown default:
                break
            }
        }
    }

    private func installTimeout(handle: UUID) {
        let timeout = DispatchWorkItem { [weak self] in
            guard let self,
                  pendingResolvers[handle] != nil
                    || tunnels[handle] != nil
                    || connections[handle] != nil
            else {
                return
            }
            cancelTunnel(handle: handle)
            eventHandler(.linkFailed(handle: handle, failure: .timedOut))
        }
        connectionTimeouts[handle] = timeout
        queue.asyncAfter(deadline: .now() + configuration.connectionTimeout, execute: timeout)
    }

    private func registerDataStream(
        _ connection: NWConnection,
        handle: UUID,
        sessionHandle: UUID,
        direction: HaloApplePeerToPeerDirection
    ) {
        dataConnections[handle] = connection
        dataStreamSessions[handle] = sessionHandle
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self,
                  let connection,
                  dataConnections[handle] === connection
            else {
                return
            }
            switch state {
            case .ready:
                guard let path = connection.currentPath,
                      HaloApplePeerToPeerNetworkPolicy.pathIsEligible(path),
                      tunnels[sessionHandle]?.authenticated == true
                else {
                    cancelDataStream(handle: handle)
                    eventHandler(.dataStreamFailed(handle: handle, failure: .ineligiblePath))
                    return
                }
                eventHandler(.dataStreamReady(
                    handle: handle,
                    sessionHandle: sessionHandle,
                    direction: direction
                ))
            case .failed:
                cancelDataStream(handle: handle)
                eventHandler(.dataStreamFailed(handle: handle, failure: .connection))
            case .cancelled:
                dataStreamSessions.removeValue(forKey: handle)
                dataConnections.removeValue(forKey: handle)
            case .setup, .preparing, .waiting:
                break
            @unknown default:
                break
            }
        }
    }

    private func cancelTunnel(handle: UUID) {
        connectionTimeouts.removeValue(forKey: handle)?.cancel()
        let resolver = pendingResolvers.removeValue(forKey: handle)
        connections.removeValue(forKey: handle)?.cancel()
        for dataHandle in dataStreamSessions
            .filter({ $0.value == handle })
            .map(\.key)
        {
            cancelDataStream(handle: dataHandle)
        }
        tunnels.removeValue(forKey: handle)?.group.cancel()
        resolver?.cancel()
    }

    private func safeParameters() throws -> NWParameters {
        let parameters = try parametersFactory()
        guard HaloApplePeerToPeerNetworkPolicy.parametersAreEligible(parameters) else {
            throw HaloApplePeerToPeerConfigurationError.unsafeNetworkParameters
        }
        return parameters
    }

    private static func presenceID(from endpoint: NWEndpoint) -> UUID? {
        guard case .service(let name, _, _, _) = endpoint else { return nil }
        return UUID(uuidString: name)
    }

    private func emitCombinedState() {
        emitState(browserReady && listenerReady ? .ready : .starting)
    }

    private func emitState(_ state: HaloApplePeerToPeerState) {
        guard lastState != state else { return }
        lastState = state
        eventHandler(.state(state))
    }

    private func stopInternal() {
        stopResources(emitStopped: started || lastState != nil)
    }

    private func stopResources(emitStopped: Bool) {
        started = false
        browserReady = false
        listenerReady = false
        browser?.cancel()
        listener?.cancel()
        let resolvers = pendingResolvers.values
        pendingResolvers.removeAll(keepingCapacity: false)
        resolvers.forEach { $0.cancel() }
        connections.values.forEach { $0.cancel() }
        dataConnections.values.forEach { $0.cancel() }
        tunnels.values.forEach { $0.group.cancel() }
        connectionTimeouts.values.forEach { $0.cancel() }
        browser = nil
        listener = nil
        connections.removeAll(keepingCapacity: false)
        dataConnections.removeAll(keepingCapacity: false)
        dataStreamSessions.removeAll(keepingCapacity: false)
        tunnels.removeAll(keepingCapacity: false)
        connectionTimeouts.removeAll(keepingCapacity: false)
        handlesByEndpoint.removeAll(keepingCapacity: false)
        endpointsByHandle.removeAll(keepingCapacity: false)
        presenceByHandle.removeAll(keepingCapacity: false)
        if emitStopped { emitState(.stopped) }
    }

    private func onQueue<T>(_ body: () throws -> T) rethrows -> T {
        if DispatchQueue.getSpecific(key: queueKey) != nil {
            return try body()
        }
        return try queue.sync(execute: body)
    }
}
