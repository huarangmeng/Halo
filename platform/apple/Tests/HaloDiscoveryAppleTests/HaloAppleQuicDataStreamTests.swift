import Foundation
import Testing

@testable import HaloDiscoveryApple

@Test func appleDataRecordLengthIsBoundedAndUsesTransferHeader() throws {
    var header = Data(repeating: 0, count: HaloAppleQuicDataProtocol.recordHeaderLength)
    header.replaceSubrange(0 ..< 4, with: Data("HDF1".utf8))
    header.replaceSubrange(28 ..< 32, with: Data([0, 0, 0, 9]))
    #expect(try haloAppleDataRecordLength(header: header) == 73)

    header.replaceSubrange(28 ..< 32, with: Data([0, 4, 0, 1]))
    #expect(throws: HaloAppleQuicDataError.invalidRecordLength(262_209)) {
        try haloAppleDataRecordLength(header: header)
    }
}

@Test func appleDataRecordLengthRejectsTruncatedHeader() {
    var header = Data(repeating: 0, count: 63)
    header.replaceSubrange(0 ..< 4, with: Data("HDF1".utf8))
    #expect(throws: HaloAppleQuicDataError.truncated) {
        try haloAppleDataRecordLength(header: header)
    }
}
