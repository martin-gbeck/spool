import Foundation
import MediaPlayer

/// Makes Read Aloud an ordinary source of audio to macOS: the play/pause,
/// next and previous keys, Control Center's player and AirPods taps all reach
/// it, and Control Center shows the chunk playing with its progress.
///
/// The engine makes the sound in its own process, so this app tells macOS
/// what is playing and passes the commands on to the engine.
@MainActor
final class MediaKeys {
    private weak var model: PlayerModel?
    private var published: NowPlayingSnapshot?

    init(model: PlayerModel) {
        self.model = model
        let center = MPRemoteCommandCenter.shared()

        handle(center.playCommand) { $0.play() }
        handle(center.pauseCommand) { $0.pause() }
        handle(center.togglePlayPauseCommand) { $0.togglePlay() }
        handle(center.nextTrackCommand) { $0.next() }
        handle(center.previousTrackCommand) { $0.previous() }
        center.skipForwardCommand.preferredIntervals = [10]
        handle(center.skipForwardCommand) { $0.seek(by: 10) }
        center.skipBackwardCommand.preferredIntervals = [10]
        handle(center.skipBackwardCommand) { $0.seek(by: -10) }
        center.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            let seconds = event.positionTime
            Task { @MainActor in self?.model?.seek(to: seconds) }
            return .success
        }
    }

    private func handle(_ command: MPRemoteCommand, _ action: @escaping @MainActor (PlayerModel) -> Void) {
        command.addTarget { [weak self] _ in
            Task { @MainActor in
                if let model = self?.model { action(model) }
            }
            return .success
        }
    }

    /// Tells macOS what is playing. Called on every poll, but only passes it on
    /// when something changed, since macOS runs the clock itself from the rate.
    func update(status: Status?, current: Entry?) {
        let center = MPNowPlayingInfoCenter.default()
        guard let status, let current else {
            center.playbackState = status == nil ? .stopped : .paused
            published = nil
            return
        }

        let now = NowPlayingSnapshot(
            id: current.id, paused: status.paused, speed: status.speed,
            duration: status.duration, position: status.position, at: Date()
        )
        guard now.differs(from: published) else { return }
        published = now

        center.nowPlayingInfo = [
            MPMediaItemPropertyTitle: Format.preview(current.text, limit: 80),
            MPMediaItemPropertyArtist: "\(current.title) · \(current.index) of \(current.of)",
            MPMediaItemPropertyAlbumTitle: "Read Aloud",
            MPMediaItemPropertyPlaybackDuration: status.duration ?? 0,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: status.position,
            MPNowPlayingInfoPropertyPlaybackRate: status.paused ? 0 : status.speed,
            MPNowPlayingInfoPropertyDefaultPlaybackRate: 1.0,
        ]
        center.playbackState = status.paused ? .paused : .playing
    }
}

/// What was last told to macOS.
struct NowPlayingSnapshot: Equatable {
    var id: UInt64
    var paused: Bool
    var speed: Double
    var duration: Double?
    var position: Double
    var at: Date

    /// True when macOS's own clock would now be wrong: another chunk, a
    /// pause, a new speed, the length known at last, or a seek, which shows
    /// as the position off by more than a second from where it should be.
    func differs(from old: NowPlayingSnapshot?) -> Bool {
        guard let old else { return true }
        if id != old.id || paused != old.paused || speed != old.speed || duration != old.duration {
            return true
        }
        let expected = old.position + (paused ? 0 : at.timeIntervalSince(old.at) * speed)
        return abs(position - expected) > 1
    }
}
