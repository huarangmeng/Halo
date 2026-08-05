import Darwin
import Testing
@testable import HaloDiscoveryApple

@Test
func ipv4SocketIsPinnedToTheRequestedInterface() throws {
    let loopbackIndex = Darwin.if_nametoindex("lo0")
    #expect(loopbackIndex != 0)
    let descriptor = try HaloAppleBoundLanSocket.makeIPv4Socket(
        interfaceIndex: loopbackIndex
    )
    defer { Darwin.close(descriptor) }

    var actualIndex: UInt32 = 0
    var actualIndexLength = socklen_t(MemoryLayout<UInt32>.size)
    let optionResult = withUnsafeMutablePointer(to: &actualIndex) { pointer in
        Darwin.getsockopt(
            descriptor,
            IPPROTO_IP,
            IP_BOUND_IF,
            pointer,
            &actualIndexLength
        )
    }
    #expect(optionResult == 0)
    #expect(actualIndex == loopbackIndex)

    var address = sockaddr_in()
    var addressLength = socklen_t(MemoryLayout<sockaddr_in>.size)
    let addressResult = withUnsafeMutablePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
            Darwin.getsockname(descriptor, socketAddress, &addressLength)
        }
    }
    #expect(addressResult == 0)
    #expect(address.sin_port != 0)
}

@Test
func zeroInterfaceIndexIsRejectedWithoutCreatingASocket() {
    #expect(throws: HaloAppleBoundLanSocketError.invalidInterface) {
        try HaloAppleBoundLanSocket.makeIPv4Socket(interfaceIndex: 0)
    }
}
