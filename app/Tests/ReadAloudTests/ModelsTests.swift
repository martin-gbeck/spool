import Foundation
import Testing
@testable import ReadAloud

/// Replies copied from the engine's actual output shape.
private let statusJSON = """
{"ok":true,"speaking":true,"paused":false,"queued":3,"loading":1,"stalled":false,
 "device":"AirPods","current":5,"position":12.5,"duration":40.0,"speed":1.25,"volume":0.8,"revision":9}
"""

private func entry(_ id: UInt64, reading: UInt64 = 1, index: Int = 1) -> Entry {
    Entry(id: id, reading: reading, title: "t", index: index, of: 3, text: "chunk \(id)", state: .ready)
}

@Test func statusDecodesFromSnakeCase() throws {
    let status = try Engine.decoder.decode(Status.self, from: Data(statusJSON.utf8))
    #expect(status.current == 5)
    #expect(status.duration == 40)
    #expect(status.speed == 1.25)
}

@Test func statusWithNothingPlayingDecodes() throws {
    let json = statusJSON
        .replacingOccurrences(of: "\"current\":5", with: "\"current\":null")
        .replacingOccurrences(of: "\"duration\":40.0", with: "\"duration\":null")
    let status = try Engine.decoder.decode(Status.self, from: Data(json.utf8))
    #expect(status.current == nil)
    #expect(status.duration == nil)
}

@Test func queueReplyDecodes() throws {
    let json = """
    {"ok":true,"queue":{"entries":[{"id":1,"reading":2,"title":"Notes","index":1,"of":2,
     "text":"Hello there.","state":"loading"}],"cursor":0},
     "speaking":false,"paused":false,"queued":1,"loading":1,"stalled":false,"device":"",
     "current":1,"position":0,"duration":null,"speed":1,"volume":1,"revision":1}
    """
    let reply = try Engine.decoder.decode(QueueReply.self, from: Data(json.utf8))
    #expect(reply.queue.entries.first?.state == .loading)
    #expect(reply.queue.current?.title == "Notes")
}

@Test func queueSplitsAroundTheCursor() {
    let queue = Queue(entries: [entry(1), entry(2), entry(3), entry(4)], cursor: 2)
    #expect(queue.current?.id == 3)
    #expect(queue.played.map(\.id) == [2, 1])
    #expect(queue.upNext.map(\.id) == [4])
}

@Test func queueAfterTheEndHasNoCurrentAndKeepsTheLastTranscript() {
    let queue = Queue(entries: [entry(1, reading: 1), entry(2, reading: 2), entry(3, reading: 2)], cursor: 3)
    #expect(queue.current == nil)
    #expect(queue.upNext.isEmpty)
    #expect(queue.transcript.map(\.id) == [2, 3])
}

@Test func transcriptHoldsOnlyTheCurrentReading() {
    let queue = Queue(entries: [entry(1, reading: 1), entry(2, reading: 2), entry(3, reading: 2)], cursor: 1)
    #expect(queue.transcript.map(\.id) == [2, 3])
    #expect(Queue.empty.transcript.isEmpty)
}

@Test func timeFormat() {
    #expect(Format.time(75) == "1:15")
    #expect(Format.time(5.9) == "0:05")
    #expect(Format.time(nil) == "0:00")
    #expect(Format.time(-3) == "0:00")
}

@Test func speedFormat() {
    #expect(Format.speed(1) == "1×")
    #expect(Format.speed(1.5) == "1.5×")
    #expect(Format.speed(1.25) == "1.25×")
}

@Test func speedStepsLikeYouTubeAndStaysInRange() {
    #expect(Format.stepped(1, by: 1) == 1.25)
    #expect(Format.stepped(1.3, by: -1) == 1)
    #expect(Format.stepped(3, by: 1) == 3)
    #expect(Format.stepped(0.5, by: -1) == 0.5)
}

@Test func previewFlattensAndCutsAtAWord() {
    #expect(Format.preview("one\n\ntwo   three") == "one two three")
    let long = String(repeating: "word ", count: 50)
    let cut = Format.preview(long, limit: 22)
    #expect(cut == "word word word word…")
}

@Test func slugMatchesThePythonLibraryNames() {
    #expect(Library.slug("DBA3803: the Presentation primer!") == "dba3803-the-presentation-primer")
    #expect(Library.slug("…") == "reading")
}
