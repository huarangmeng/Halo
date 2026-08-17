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
              path.supportsIPv4,
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

    public static func makeIPv4Socket(on interface: NWInterface) throws -> Int32 {
        guard interface.type == .wifi || interface.type == .wiredEthernet,
              let index = UInt32(exactly: interface.index),
              index != 0
        else {
            throw HaloAppleBoundLanSocketError.invalidInterface
        }
        return try makeIPv4Socket(interfaceIndex: index)
    }

    static func makeIPv4Socket(interfaceIndex: UInt32) throws -> Int32 {
        guard interfaceIndex != 0 else {
            throw HaloAppleBoundLanSocketError.invalidInterface
        }
        let descriptor = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard descriptor >= 0 else {
            throw HaloAppleBoundLanSocketError.socketCreationFailed
        }
        do {
            var boundInterface = interfaceIndex
            let optionResult = withUnsafePointer(to: &boundInterface) { pointer in
                Darwin.setsockopt(
                    descriptor,
                    IPPROTO_IP,
                    IP_BOUND_IF,
                    pointer,
                    socklen_t(MemoryLayout<UInt32>.size)
                )
            }
            guard optionResult == 0 else {
                throw HaloAppleBoundLanSocketError.interfaceBindingFailed
            }

            var address = sockaddr_in()
            address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            address.sin_family = sa_family_t(AF_INET)
            address.sin_port = in_port_t(0)
            address.sin_addr = in_addr(s_addr: in_addr_t(0))
            let bindResult = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                    Darwin.bind(
                        descriptor,
                        socketAddress,
                        socklen_t(MemoryLayout<sockaddr_in>.size)
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
