import Foundation

enum EngineError: LocalizedError {
    case notRunning
    case noReply
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .notRunning: "The engine is not running."
        case .noReply: "The engine did not answer."
        case .failed(let message): message
        }
    }
}

/// Talks to the Rust engine over the same unix socket Claude's tts_speak uses:
/// one JSON object per line each way, a new connection per command.
struct Engine: Sendable {
    static let directory = FileManager.default.homeDirectoryForCurrentUser
        .appending(path: "Library/Application Support/rust_tts")
    static let socketPath = directory.appending(path: "rust_tts.sock").path
    static let logURL = directory.appending(path: "rust_tts.log")

    /// Sends one command and returns the raw reply line. Blocking, so callers
    /// run it off the main thread.
    func roundTrip(_ payload: Data) throws -> Data {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw EngineError.notRunning }
        defer { close(fd) }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let path = Array(Self.socketPath.utf8CString)
        guard path.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw EngineError.notRunning }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            path.withUnsafeBytes { buffer.copyMemory(from: $0) }
        }

        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { throw EngineError.notRunning }

        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))

        var line = payload
        line.append(0x0A)
        let sent = line.withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
        guard sent == line.count else { throw EngineError.noReply }

        var reply = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while !reply.contains(0x0A) {
            let count = read(fd, &buffer, buffer.count)
            guard count > 0 else { break }
            reply.append(buffer, count: count)
        }
        guard !reply.isEmpty else { throw EngineError.noReply }
        return reply
    }

    func send<Reply: Decodable & Sendable>(_ command: [String: Any], as type: Reply.Type) async throws -> Reply {
        let payload = try JSONSerialization.data(withJSONObject: command)
        let reply = try await Task.detached { try roundTrip(payload) }.value
        return try Self.decoder.decode(Reply.self, from: reply)
    }

    static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()

    /// Starts the engine bundled inside this app. Started from here, macOS
    /// counts Spool as responsible for it, so the Accessibility grant
    /// Ctrl+Space needs is the one Spool gets.
    func launch() throws {
        let url = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/rust-tts")
        guard FileManager.default.isExecutableFile(atPath: url.path) else {
            throw EngineError.failed("rust-tts is missing from the app bundle. Rebuild with build.sh.")
        }

        try FileManager.default.createDirectory(at: Self.directory, withIntermediateDirectories: true)
        // O_APPEND, not a seek to the end: a seeked handle writes from a fixed
        // offset, so the engine overwrote the log in place instead of adding to it.
        let fd = open(Self.logURL.path, O_WRONLY | O_APPEND | O_CREAT, 0o644)
        guard fd >= 0 else { throw EngineError.failed("Cannot open \(Self.logURL.path)") }
        let log = FileHandle(fileDescriptor: fd, closeOnDealloc: true)

        let process = Process()
        process.executableURL = url
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = log
        process.standardError = log
        try process.run()
    }

    /// The last lines the engine wrote, for when it will not start.
    static func logTail(lines: Int = 4) -> String {
        guard let text = try? String(contentsOf: logURL, encoding: .utf8) else { return "" }
        return text.split(separator: "\n").suffix(lines).joined(separator: "\n")
    }
}
