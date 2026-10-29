import Foundation

/// The saved readings tts.py keeps: one folder per reading with transcript.md,
/// the engine adding 001.mp3 and 001.txt per chunk. Same layout, so tts_replay
/// and tts_library find pasted text too.
enum Library {
    static let directory = Engine.directory.appending(path: "library")

    /// Lowercase words joined by hyphens, as tts.py's _slug does.
    static func slug(_ text: String, words: Int = 8) -> String {
        let found = text.lowercased()
            .split(whereSeparator: { !($0.isASCII && ($0.isLetter || $0.isNumber)) })
            .prefix(words)
        return found.isEmpty ? "reading" : found.joined(separator: "-")
    }

    static func newReading(for text: String, at date: Date = .now) throws -> URL {
        let stamp = date.formatted(.verbatim(
            "\(year: .defaultDigits)-\(month: .twoDigits)-\(day: .twoDigits)-\(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased))\(minute: .twoDigits)",
            timeZone: .current, calendar: .current
        ))
        let folder = directory.appending(path: "\(stamp)-\(slug(text))")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)

        let heading = text.split(whereSeparator: \.isWhitespace).prefix(8).joined(separator: " ")
        try "# \(heading)\n\n\(text)\n".write(to: folder.appending(path: "transcript.md"), atomically: true, encoding: .utf8)
        return folder
    }
}
