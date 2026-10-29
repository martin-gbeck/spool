import AppKit
import ApplicationServices
import Observation

/// The app's view of the engine. Polls status four times a second and
/// refetches the queue only when the engine says the list changed.
@MainActor
@Observable
final class PlayerModel {
    private(set) var status: Status?
    private(set) var queue = Queue.empty
    private(set) var settings: EngineSettings?
    /// Set while the engine is down, with the reason when there is one.
    private(set) var engineProblem: String?
    private(set) var hotkeyAllowed = AXIsProcessTrusted()
    /// The scrubber's position while he drags it, so polling does not yank it back.
    var scrubbing: Double?

    private let engine = Engine()
    private var lastRevision: UInt64?
    private var lastLaunch = Date.distantPast
    private var polling: Task<Void, Never>?

    init() {
        polling = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                try? await Task.sleep(for: .milliseconds(250))
            }
        }
    }

    var current: Entry? { queue.current }
    var isPlaying: Bool { status.map { !$0.paused && $0.current != nil } ?? false }
    var speed: Double { status?.speed ?? 1 }

    func refresh() async {
        do {
            let status = try await engine.send(["cmd": "status"], as: Status.self)
            self.status = status
            engineProblem = nil

            if status.revision != lastRevision {
                queue = try await engine.send(["cmd": "queue"], as: QueueReply.self).queue
                lastRevision = status.revision
            }
            if settings == nil {
                settings = try await engine.send(["cmd": "settings"], as: SettingsReply.self).settings
            }
            hotkeyAllowed = AXIsProcessTrusted()
        } catch {
            status = nil
            lastRevision = nil
            startEngineIfDue()
        }
    }

    /// Starts the engine at most every five seconds, so one that crashes on
    /// start does not get relaunched four times a second.
    private func startEngineIfDue() {
        guard Date().timeIntervalSince(lastLaunch) > 5 else { return }
        lastLaunch = Date()
        do {
            try engine.launch()
            engineProblem = "Starting the engine…"
        } catch {
            engineProblem = error.localizedDescription
        }
        Task {
            try? await Task.sleep(for: .seconds(3))
            if status == nil {
                let tail = Engine.logTail()
                engineProblem = tail.isEmpty ? "The engine did not start." : "The engine did not start:\n\(tail)"
            }
        }
    }

    private func send(_ command: [String: Any]) {
        Task {
            _ = try? await engine.send(command, as: Status.self)
            await refresh()
        }
    }

    // MARK: Transport

    func togglePlay() { send(["cmd": "toggle"]) }
    func next() { send(["cmd": "next"]) }
    func previous() { send(["cmd": "previous"]) }
    func restart() { send(["cmd": "restart"]) }
    func seek(by seconds: Double) { send(["cmd": "seek", "by": seconds]) }

    func seek(to seconds: Double) {
        send(["cmd": "seek", "to": seconds])
        // Held until the engine reports the new position, so the knob does not flick back.
        Task {
            try? await Task.sleep(for: .milliseconds(400))
            scrubbing = nil
        }
    }

    /// Queues text behind whatever is playing, chunked as the hotkey's is.
    /// The engine saves it to the library like every other reading.
    func speak(_ text: String) { send(["cmd": "speak", "text": text]) }

    /// Puts `id` just before `before`, or at the end of the queue.
    func move(_ id: UInt64, before: UInt64?) {
        var command: [String: Any] = ["cmd": "move", "id": id]
        if let before { command["before"] = before }
        send(command)
    }

    func jump(to entry: Entry) { send(["cmd": "jump", "id": entry.id]) }
    func remove(_ entry: Entry) { send(["cmd": "remove", "id": entry.id]) }
    func clearPlayed() { send(["cmd": "clear_played"]) }
    func stop() { send(["cmd": "stop"]) }

    func setSpeed(_ value: Double) {
        send(["cmd": "set_speed", "speed": min(Format.maxSpeed, max(Format.minSpeed, value))])
    }

    func faster() { setSpeed(Format.stepped(speed, by: 1)) }
    func slower() { setSpeed(Format.stepped(speed, by: -1)) }

    func setVolume(_ value: Double) { send(["cmd": "set_volume", "volume": value]) }

    // MARK: Settings

    func change(voice: String? = nil, model: String? = nil, instructions: String? = nil) {
        var command: [String: Any] = ["cmd": "set_settings"]
        if let voice { command["voice"] = voice }
        if let model { command["model"] = model }
        if let instructions { command["instructions"] = instructions }
        Task {
            if let reply = try? await engine.send(command, as: SettingsReply.self) {
                settings = reply.settings
            }
        }
    }

    /// Speaks a sample in `voice` so he can hear it before choosing.
    func preview(voice: String) {
        send([
            "cmd": "speak",
            "text": "This is the \(voice) voice. Here is how a normal sentence from your notes would sound.",
            "voice": voice,
            "title": "Voice preview: \(voice)",
        ])
    }

    /// Quits the engine; the next poll starts a fresh one. Needed after
    /// granting Accessibility, since the engine checks it only at start.
    func restartEngine() {
        lastLaunch = .distantPast
        send(["cmd": "quit"])
    }

    func quitEngine() {
        Task { _ = try? await engine.send(["cmd": "quit"], as: Status.self) }
    }

    func openAccessibilitySettings() {
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
}
