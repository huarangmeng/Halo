import Foundation
import Testing

@testable import HaloDiscoveryApple

@Test func appleDataRecordLengthIsBoundedAndUsesTransferHeader() throws {
    var header = Data(repeating: 0, count: HaloAppleQuicDataProtocol.recordHeaderLength)
    header.replaceSubrange(24 ..< 28, with: [0, 0, 0, 32])
    #expect(try haloAppleDataRecordLength(header: header) == 92)

    header.replaceSubrange(24 ..< 28, with: [0, 4, 0, 1])
    #expect(throws: HaloAppleQuicDataError.invalidRecordLength(262_205)) {
        try haloAppleDataRecordLength(header: header)
    }
}

@Test func appleDataRecordLengthRejectsTruncatedHeader() {
    #expect(throws: HaloAppleQuicDataError.truncated) {
        try haloAppleDataRecordLength(header: Data(repeating: 0, count: 59))
    }
}
