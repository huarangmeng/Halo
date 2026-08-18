import Darwin
import Foundation
import Network

public enum HaloAppleBoundLanSocketError: Error, Equatable {
    case ineligiblePath
    case invalidInterface
    case socketCreationFailed
    case interfaceBindingFailed
    case addressBindingFailed
}

public enum HaloAppleLocalNetworkScope: Equatable {
    case shared
    case userApprovedHotspot
}

/// Creates a UDP socket whose packets cannot leave the selected Apple network
/// interface. The caller owns the returned descriptor until it explicitly
/// transfers that ownership to Rust.
public enum HaloAppleBoundLanSocket {
    public static func eligibleInterface(
        on path: NWPath,
        scope: HaloAppleLocalNetworkScope = .shared
    ) -> NWInterface? {
        guard path.status == .satisfied,
              (path.supportsIPv4 || path.supportsIPv6),
              allows(
                  isExpensive: path.isExpensive,
                  isConstrained: path.isConstrained,
                  scope: scope
              )
        else {
            return nil
        }
        return path.availableInterfaces.first { interface in
            guard path.usesInterfaceType(interface.type) else { return false }
            return switch scope {
            case .shared:
                interface.type == .wifi || interface.type == .wiredEthernet
            case .userApprovedHotspot:
                interface.type == .wifi
            }
        }
    }

    public static func allows(
        isExpensive: Bool,
        isConstrained: Bool,
        scope: HaloAppleLocalNetworkScope
    ) -> Bool {
        switch scope {
        case .shared:
            !isExpensive && !isConstrained
        case .userApprovedHotspot:
            true
        }
    }

    public static func makeDualStackSocket(on interface: NWInterface) throws -> Int32 {
        guard interface.type == .wifi || interface.type == .wiredEthernet,
              let index = UInt32(exactly: interface.index),
              index != 0
        else {
            throw HaloAppleBoundLanSocketError.invalidInterface
        }
        return try makeDualStackSocket(interfaceIndex: index)
    }

    static func makeDualStackSocket(interfaceIndex: UInt32) throws -> Int32 {
        guard interfaceIndex != 0 else {
            throw HaloAppleBoundLanSocketError.invalidInterface
        }
        let descriptor = Darwin.socket(AF_INET6, SOCK_DGRAM, IPPROTO_UDP)
        guard descriptor >= 0 else {
            throw HaloAppleBoundLanSocketError.socketCreationFailed
        }
        do {
            var boundInterface = interfaceIndex
            let optionResult = withUnsafePointer(to: &boundInterface) { pointer in
                Darwin.setsockopt(
                    descriptor,
                    IPPROTO_IPV6,
                    IPV6_BOUND_IF,
                    pointer,
                    socklen_t(MemoryLayout<UInt32>.size)
                )
            }
            guard optionResult == 0 else {
                throw HaloAppleBoundLanSocketError.interfaceBindingFailed
            }

            var ipv6Only: Int32 = 0
            let dualStackResult = withUnsafePointer(to: &ipv6Only) { pointer in
                Darwin.setsockopt(
                    descriptor,
                    IPPROTO_IPV6,
                    IPV6_V6ONLY,
                    pointer,
                    socklen_t(MemoryLayout<Int32>.size)
                )
            }
            guard dualStackResult == 0 else {
                throw HaloAppleBoundLanSocketError.socketCreationFailed
            }

            var address = sockaddr_in6()
            address.sin6_len = UInt8(MemoryLayout<sockaddr_in6>.size)
            address.sin6_family = sa_family_t(AF_INET6)
            address.sin6_port = in_port_t(0)
            address.sin6_addr = in6addr_any
            let bindResult = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                    Darwin.bind(
                        descriptor,
                        socketAddress,
                        socklen_t(MemoryLayout<sockaddr_in6>.size)
                    )
                }
            }
            guard bindResult == 0 else {
                throw HaloAppleBoundLanSocketError.addressBindingFailed
            }
            guard Darwin.fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0 else {
                throw HaloAppleBoundLanSocketError.socketCreationFailed
            }
            return descriptor
        } catch {
            Darwin.close(descriptor)
            throw error
        }
    }
}
