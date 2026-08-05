import Foundation
import Network
import dnssd

enum HaloBonjourServiceResolverError: Error, Equatable {
    case invalidServiceEndpoint
    case startFailed(Int32)
    case dispatchFailed(Int32)
    case resolveFailed(Int32)
    case invalidResult
    case cancelled
}

struct HaloResolvedBonjourEndpoint: Equatable, Sendable {
    let host: String
    let port: UInt16

    var networkEndpoint: NWEndpoint? {
        guard !host.isEmpty, let port = NWEndpoint.Port(rawValue: port) else { return nil }
        return .hostPort(host: NWEndpoint.Host(host), port: port)
    }
}

func haloBonjourHostPort(
    host: UnsafePointer<CChar>?,
    networkOrderPort: UInt16
) -> HaloResolvedBonjourEndpoint? {
    guard let host else { return nil }
    let name = String(cString: host)
    let port = UInt16(bigEndian: networkOrderPort)
    guard !name.isEmpty, port != 0 else { return nil }
    return HaloResolvedBonjourEndpoint(host: name, port: port)
}

/// Resolves an already-discovered Bonjour service to the host/port endpoint
/// required by `NWMultiplexGroup`. The browser must remain active so the
/// peer-to-peer association remains owned by mDNSResponder.
final class HaloBonjourServiceResolver: @unchecked Sendable {
    typealias Completion = @Sendable (Result<HaloResolvedBonjourEndpoint, Error>) -> Void

    private let name: String
    private let type: String
    private let domain: String
    private let interfaceIndex: UInt32
    private let queue: DispatchQueue
    private let completion: Completion
    private var serviceRef: DNSServiceRef?
    private var finished = false

    init(
        endpoint: NWEndpoint,
        queue: DispatchQueue,
        completion: @escaping Completion
    ) throws {
        guard case .service(let name, let type, let domain, let interface) = endpoint,
              !name.isEmpty,
              !type.isEmpty,
              !domain.isEmpty
        else {
            throw HaloBonjourServiceResolverError.invalidServiceEndpoint
        }
        self.name = name
        self.type = type
        self.domain = domain
        interfaceIndex = UInt32(interface?.index ?? 0)
        self.queue = queue
        self.completion = completion
    }

    func start() throws {
        precondition(serviceRef == nil && !finished)
        var reference: DNSServiceRef?
        let error = DNSServiceResolve(
            &reference,
            DNSServiceFlags(kDNSServiceFlagsIncludeP2P),
            interfaceIndex,
            name,
            type,
            domain,
            { _, _, _, errorCode, _, hostTarget, port, _, _, context in
                guard let context else { return }
                let resolver = Unmanaged<HaloBonjourServiceResolver>
                    .fromOpaque(context)
                    .takeUnretainedValue()
                guard errorCode == kDNSServiceErr_NoError else {
                    resolver.finish(.failure(
                        HaloBonjourServiceResolverError.resolveFailed(errorCode)
                    ))
                    return
                }
                guard let endpoint = haloBonjourHostPort(
                    host: hostTarget,
                    networkOrderPort: port
                ) else {
                    resolver.finish(.failure(HaloBonjourServiceResolverError.invalidResult))
                    return
                }
                resolver.finish(.success(endpoint))
            },
            Unmanaged.passUnretained(self).toOpaque()
        )
        guard error == kDNSServiceErr_NoError, let reference else {
            throw HaloBonjourServiceResolverError.startFailed(error)
        }
        serviceRef = reference
        let dispatchError = DNSServiceSetDispatchQueue(reference, queue)
        guard dispatchError == kDNSServiceErr_NoError else {
            DNSServiceRefDeallocate(reference)
            serviceRef = nil
            throw HaloBonjourServiceResolverError.dispatchFailed(dispatchError)
        }
    }

    func cancel() {
        finish(.failure(HaloBonjourServiceResolverError.cancelled))
    }

    private func finish(_ result: Result<HaloResolvedBonjourEndpoint, Error>) {
        guard !finished else { return }
        finished = true
        if let serviceRef {
            DNSServiceRefDeallocate(serviceRef)
            self.serviceRef = nil
        }
        completion(result)
    }
}
