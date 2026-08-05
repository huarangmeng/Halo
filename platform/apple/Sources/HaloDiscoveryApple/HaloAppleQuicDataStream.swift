@preconcurrency import Network
import Foundation

public enum HaloAppleQuicDataProtocol {
    public static let recordHeaderLength = 60
    public static let maximumPayloadLength = 256 * 1_024
    public static let maximumRecordLength = recordHeaderLength + maximumPayloadLength
}

public enum HaloAppleQuicDataError: Error, Equatable {
    case ineligiblePath
    case exporterMismatch
    case invalidRecordLength(Int)
    case truncated
    case readFailed
    case writeFailed
    case concurrentOperation
    case closed
}

func haloAppleDataRecordLength(header: Data) throws -> Int {
    guard header.count == HaloAppleQuicDataProtocol.recordHeaderLength else {
        throw HaloAppleQuicDataError.truncated
    }
    let payloadLength = header[24 ..< 28].reduce(0) { partial, byte in
        (partial << 8) | Int(byte)
    }
    let (recordLength, overflow) =
        HaloAppleQuicDataProtocol.recordHeaderLength.addingReportingOverflow(payloadLength)
    guard !overflow,
          payloadLength <= HaloAppleQuicDataProtocol.maximumPayloadLength,
          recordLength <= HaloAppleQuicDataProtocol.maximumRecordLength
    else {
        throw HaloAppleQuicDataError.invalidRecordLength(recordLength)
    }
    return recordLength
}

/// A file-data QUIC stream opened only from an exporter-authenticated tunnel.
/// Complete bounded records cross the platform boundary; Rust remains the sole
/// parser and validates magic, transfer ID, index, digest, and state.
public actor HaloAppleQuicDataStream {
    private let connection: NWConnection
    private var receiving = false
    private var sending = false
    private var isClosed = false

    public init(connection: NWConnection, expectedChannelBinding: Data) throws {
        guard let path = connection.currentPath,
              HaloApplePeerToPeerNetworkPolicy.pathIsEligible(path)
        else {
            throw HaloAppleQuicDataError.ineligiblePath
        }
        guard let metadata = connection.metadata(definition: NWProtocolQUIC.definition)
            as? NWProtocolQUIC.Metadata,
              try HaloAppleQuicControlChannel.exportChannelBinding(
                  metadata: metadata.securityProtocolMetadata
              ) == expectedChannelBinding
        else {
            throw HaloAppleQuicDataError.exporterMismatch
        }
        self.connection = connection
    }

    public func sendRecord(_ record: Data) async throws {
        guard !isClosed else { throw HaloAppleQuicDataError.closed }
        guard !sending else { throw HaloAppleQuicDataError.concurrentOperation }
        guard record.count >= HaloAppleQuicDataProtocol.recordHeaderLength,
              record.count <= HaloAppleQuicDataProtocol.maximumRecordLength,
              try haloAppleDataRecordLength(
                  header: record.prefix(HaloAppleQuicDataProtocol.recordHeaderLength)
              ) == record.count
        else {
            throw HaloAppleQuicDataError.invalidRecordLength(record.count)
        }
        sending = true
        defer { sending = false }
        try await withCheckedThrowingContinuation { continuation in
            connection.send(
                content: record,
                contentContext: .defaultStream,
                isComplete: false,
                completion: .contentProcessed { error in
                    if error == nil {
                        continuation.resume()
                    } else {
                        continuation.resume(throwing: HaloAppleQuicDataError.writeFailed)
                    }
                }
            )
        }
    }

    public func receiveRecord() async throws -> Data {
        guard !isClosed else { throw HaloAppleQuicDataError.closed }
        guard !receiving else { throw HaloAppleQuicDataError.concurrentOperation }
        receiving = true
        defer { receiving = false }
        let header = try await receiveExactly(HaloAppleQuicDataProtocol.recordHeaderLength)
        let recordLength = try haloAppleDataRecordLength(header: header)
        let payloadLength = recordLength - HaloAppleQuicDataProtocol.recordHeaderLength
        if payloadLength == 0 { return header }
        let payload = try await receiveExactly(payloadLength)
        var record = Data(capacity: recordLength)
        record.append(header)
        record.append(payload)
        return record
    }

    public func close() {
        guard !isClosed else { return }
        isClosed = true
        connection.cancel()
    }

    private func receiveExactly(_ length: Int) async throws -> Data {
        var result = Data(capacity: length)
        while result.count < length {
            let remaining = length - result.count
            let chunk = try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<(Data, Bool), Error>) in
                connection.receive(
                    minimumIncompleteLength: 1,
                    maximumLength: remaining
                ) { content, _, complete, error in
                    if error != nil {
                        continuation.resume(throwing: HaloAppleQuicDataError.readFailed)
                    } else {
                        continuation.resume(returning: (content ?? Data(), complete))
                    }
                }
            }
            guard !chunk.0.isEmpty else {
                throw chunk.1 ? HaloAppleQuicDataError.truncated : HaloAppleQuicDataError.readFailed
            }
            result.append(chunk.0)
            if chunk.1, result.count < length { throw HaloAppleQuicDataError.truncated }
        }
        return result
    }
}
