@preconcurrency import Network
import Foundation
import Security

public enum HaloAppleQuicControlProtocol {
    public static let frameHeaderLength = 12
    public static let maximumFrameLength = 4_096
    public static let exporterLabel = "EXPORTER-Halo-Pairing-v1"
    public static let exporterLength = 32
}

public enum HaloAppleQuicControlError: Error, Equatable {
    case ineligiblePath
    case missingQuicMetadata
    case unexpectedAlpn
    case earlyDataAccepted
    case exporterUnavailable
    case invalidFrameLength(Int)
    case truncated
    case readFailed
    case writeFailed
    case concurrentOperation
    case closed
}

/// Returns the complete frame length declared by a Halo control header.
/// Native code reads only this length field; Rust remains the sole protocol
/// parser and validates magic, version, kind, flags, and message contents.
func haloAppleControlFrameLength(
    header: Data,
    maximumLength: Int = HaloAppleQuicControlProtocol.maximumFrameLength
) throws -> Int {
    guard header.count == HaloAppleQuicControlProtocol.frameHeaderLength else {
        throw HaloAppleQuicControlError.truncated
    }
    let payloadLength = header[8 ..< 12].reduce(0) { partial, byte in
        (partial << 8) | Int(byte)
    }
    let (frameLength, overflow) =
        HaloAppleQuicControlProtocol.frameHeaderLength.addingReportingOverflow(payloadLength)
    guard !overflow,
          frameLength >= HaloAppleQuicControlProtocol.frameHeaderLength,
          frameLength <= maximumLength,
          maximumLength <= HaloAppleQuicControlProtocol.maximumFrameLength
    else {
        throw HaloAppleQuicControlError.invalidFrameLength(frameLength)
    }
    return frameLength
}

/// One bounded bidirectional Halo control stream carried by Network.framework
/// QUIC. The object never crosses into Dart. Its 32-byte exporter and complete
/// frames are the only values passed to the Rust pairing state machine.
public actor HaloAppleQuicControlChannel {
    public nonisolated let channelBinding: Data

    private let connection: NWConnection
    private var receiving = false
    private var sending = false
    private var isClosed = false

    public init(connection: NWConnection) throws {
        guard let path = connection.currentPath,
              HaloApplePeerToPeerNetworkPolicy.pathIsEligible(path)
        else {
            throw HaloAppleQuicControlError.ineligiblePath
        }
        guard let metadata = connection.metadata(definition: NWProtocolQUIC.definition)
            as? NWProtocolQUIC.Metadata
        else {
            throw HaloAppleQuicControlError.missingQuicMetadata
        }
        guard metadata.negotiatedALPN == HaloApplePeerToPeerProtocol.alpn else {
            throw HaloAppleQuicControlError.unexpectedAlpn
        }
        guard !sec_protocol_metadata_get_early_data_accepted(
            metadata.securityProtocolMetadata
        ) else {
            throw HaloAppleQuicControlError.earlyDataAccepted
        }
        channelBinding = try Self.exportChannelBinding(
            metadata: metadata.securityProtocolMetadata
        )
        self.connection = connection
    }

    public func sendFrame(_ frame: Data) async throws {
        guard !isClosed else { throw HaloAppleQuicControlError.closed }
        guard !sending else { throw HaloAppleQuicControlError.concurrentOperation }
        guard frame.count >= HaloAppleQuicControlProtocol.frameHeaderLength,
              frame.count <= HaloAppleQuicControlProtocol.maximumFrameLength
        else {
            throw HaloAppleQuicControlError.invalidFrameLength(frame.count)
        }
        let declaredLength = try haloAppleControlFrameLength(
            header: frame.prefix(HaloAppleQuicControlProtocol.frameHeaderLength)
        )
        guard declaredLength == frame.count else {
            throw HaloAppleQuicControlError.invalidFrameLength(frame.count)
        }

        sending = true
        defer { sending = false }
        try await withCheckedThrowingContinuation { continuation in
            connection.send(
                content: frame,
                contentContext: .defaultStream,
                isComplete: false,
                completion: .contentProcessed { error in
                    if error == nil {
                        continuation.resume()
                    } else {
                        continuation.resume(throwing: HaloAppleQuicControlError.writeFailed)
                    }
                }
            )
        }
    }

    public func receiveFrame(
        maximumLength: Int = HaloAppleQuicControlProtocol.maximumFrameLength
    ) async throws -> Data {
        guard !isClosed else { throw HaloAppleQuicControlError.closed }
        guard !receiving else { throw HaloAppleQuicControlError.concurrentOperation }
        guard maximumLength >= HaloAppleQuicControlProtocol.frameHeaderLength,
              maximumLength <= HaloAppleQuicControlProtocol.maximumFrameLength
        else {
            throw HaloAppleQuicControlError.invalidFrameLength(maximumLength)
        }

        receiving = true
        defer { receiving = false }
        let header = try await receiveExactly(HaloAppleQuicControlProtocol.frameHeaderLength)
        let frameLength = try haloAppleControlFrameLength(
            header: header,
            maximumLength: maximumLength
        )
        let payloadLength = frameLength - HaloAppleQuicControlProtocol.frameHeaderLength
        if payloadLength == 0 { return header }
        let payload = try await receiveExactly(payloadLength)
        var frame = Data(capacity: frameLength)
        frame.append(header)
        frame.append(payload)
        return frame
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
            let chunk = try await receiveChunk(maximumLength: remaining)
            guard !chunk.data.isEmpty else {
                throw chunk.complete
                    ? HaloAppleQuicControlError.truncated
                    : HaloAppleQuicControlError.readFailed
            }
            result.append(chunk.data)
            if chunk.complete, result.count < length {
                throw HaloAppleQuicControlError.truncated
            }
        }
        return result
    }

    private func receiveChunk(maximumLength: Int) async throws -> (data: Data, complete: Bool) {
        try await withCheckedThrowingContinuation { continuation in
            connection.receive(
                minimumIncompleteLength: 1,
                maximumLength: maximumLength
            ) { content, _, isComplete, error in
                if error != nil {
                    continuation.resume(throwing: HaloAppleQuicControlError.readFailed)
                } else {
                    continuation.resume(returning: (content ?? Data(), isComplete))
                }
            }
        }
    }

    static func exportChannelBinding(
        metadata: sec_protocol_metadata_t
    ) throws -> Data {
        let label = HaloAppleQuicControlProtocol.exporterLabel
        let secret = label.withCString { pointer in
            sec_protocol_metadata_create_secret(
                metadata,
                label.utf8.count,
                pointer,
                HaloAppleQuicControlProtocol.exporterLength
            )
        }
        guard let secret else {
            throw HaloAppleQuicControlError.exporterUnavailable
        }
        let dispatchData = secret as DispatchData
        let binding = Data(dispatchData)
        guard binding.count == HaloAppleQuicControlProtocol.exporterLength else {
            throw HaloAppleQuicControlError.exporterUnavailable
        }
        return binding
    }
}
