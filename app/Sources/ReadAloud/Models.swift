import Foundation

/// What the engine says on every reply. Field names follow the engine's JSON
/// in snake_case, converted by the decoder.
struct Status: Decodable, Equatable {
    var ok: Bool
    var error: String?
    var speaking: Bool
    var paused: Bool
    var queued: Int
    var loading: Int
    var stalled: Bool
    var device: String
    var current: UInt64?
    var position: Double
    var duration: Double?
    var speed: Double
    var volume: Double
    var revision: UInt64
}

struct Entry: Decodable, Identifiable, Equatable {
    enum State: String, Decodable {
        case loading, ready, failed
    }

    var id: UInt64
    var reading: UInt64
    var title: String
    var index: Int
    var of: Int
    var text: String
    var state: State
}

struct Queue: Decodable, Equatable {
    var entries: [Entry]
    /// Index of the playing entry. Equal to the count when everything has played.
    var cursor: Int

    static let empty = Queue(entries: [], cursor: 0)

    var current: Entry? {
        entries.indices.contains(cursor) ? entries[cursor] : nil
    }

    /// Most recent first, since the chunk that just played is the one he looks for.
    var played: [Entry] {
        Array(entries.prefix(min(cursor, entries.count)).reversed())
    }

    var upNext: [Entry] {
        cursor + 1 < entries.count ? Array(entries[(cursor + 1)...]) : []
    }

    /// Every chunk of the reading that is playing, or of the last one played.
    var transcript: [Entry] {
        guard let reading = (current ?? entries.prefix(cursor).last)?.reading else { return [] }
        return entries.filter { $0.reading == reading }
    }
}

struct QueueReply: Decodable {
    var queue: Queue
}

struct EngineSettings: Decodable, Equatable {
    var voice: String
    var model: String
    var instructions: String
    var shortcut: String
    var speed: Double
    var volume: Double
    var voices: [String]
    var models: [String]
}

struct SettingsReply: Decodable {
    var ok: Bool
    var error: String?
    var settings: EngineSettings
}

enum Format {
    static let speeds: [Double] = [0.75, 1, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 3]
    static let minSpeed = 0.5
    static let maxSpeed = 3.0
    /// One press of faster or slower, as on YouTube.
    static let speedStep = 0.25

    /// 75 -> "1:15". Negative and missing values show as 0:00.
    static func time(_ seconds: Double?) -> String {
        let total = Int(max(0, seconds ?? 0).rounded(.down))
        return String(format: "%d:%02d", total / 60, total % 60)
    }

    /// 1.5 -> "1.5×", 1 -> "1×", 1.25 -> "1.25×".
    static func speed(_ value: Double) -> String {
        var text = String(format: "%.2f", value)
        while text.hasSuffix("0") { text.removeLast() }
        if text.hasSuffix(".") { text.removeLast() }
        return text + "×"
    }

    /// The opening words of a chunk on one line, cut at a word near `limit` characters.
    static func preview(_ text: String, limit: Int = 140) -> String {
        let flat = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        guard flat.count > limit else { return flat }
        let cut = flat.prefix(limit)
        let end = cut.lastIndex(of: " ") ?? cut.endIndex
        return String(cut[..<end]) + "…"
    }

    static func stepped(_ speed: Double, by direction: Double) -> Double {
        let next = (speed / speedStep).rounded() * speedStep + direction * speedStep
        return min(maxSpeed, max(minSpeed, next))
    }
}

/// Where new speech goes in the queue. The engine takes the raw value.
enum Place: String {
    /// Plays at once; the interrupted chunk waits at the top of up next.
    case now
    /// Plays when the current chunk ends.
    case next
    /// After everything queued.
    case last
}
