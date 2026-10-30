import SwiftUI

@main
struct ReadAloudApp: App {
    @State private var model = PlayerModel()

    init() {
        Snapshot.scheduleIfAsked()
    }

    var body: some Scene {
        Window("Spool", id: "main") {
            PlayerView()
                .environment(model)
                .frame(minWidth: 900, minHeight: 560)
        }
        .defaultSize(width: 1180, height: 720)
        .commands { PlaybackCommands(model: model) }

        MenuBarExtra {
            MiniPlayerView()
                .environment(model)
        } label: {
            Image(systemName: model.isPlaying ? "speaker.wave.3.fill" : "speaker.wave.2")
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environment(model)
        }
    }
}

/// The Playback menu. Every item has a Command shortcut so it works anywhere;
/// the single-key ones (space, J, K, L, arrows) live on the player window.
struct PlaybackCommands: Commands {
    let model: PlayerModel

    var body: some Commands {
        CommandGroup(after: .newItem) {
            Button("Add Audio…") { model.chooseAudio() }
                .keyboardShortcut("o")
        }
        CommandMenu("Playback") {
            Button(model.isPlaying ? "Pause" : "Play") { model.togglePlay() }
                .keyboardShortcut("p")
            Button("Restart Chunk") { model.restart() }
                .keyboardShortcut("r")
            Divider()
            Button("Back 10 Seconds") { model.seek(by: -10) }
                .keyboardShortcut(.leftArrow)
            Button("Forward 10 Seconds") { model.seek(by: 10) }
                .keyboardShortcut(.rightArrow)
            Button("Previous Chunk") { model.previous() }
                .keyboardShortcut(.leftArrow, modifiers: [.command, .shift])
            Button("Next Chunk") { model.next() }
                .keyboardShortcut(.rightArrow, modifiers: [.command, .shift])
            Divider()
            Button("Faster") { model.faster() }
                .keyboardShortcut("]")
            Button("Slower") { model.slower() }
                .keyboardShortcut("[")
            Button("Normal Speed") { model.setSpeed(1) }
                .keyboardShortcut("0")
            Divider()
            Button("Stop and Clear Up Next") { model.stop() }
                .keyboardShortcut(".")
        }
    }
}

/// With READ_ALOUD_SNAPSHOT=/path.png set, writes the main window to that file
/// a few seconds after launch. How the window gets checked after a change
/// without screen-recording permission.
enum Snapshot {
    @MainActor static func scheduleIfAsked() {
        guard let path = ProcessInfo.processInfo.environment["READ_ALOUD_SNAPSHOT"] else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            guard let view = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 500 })?.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(filePath: path))
        }
    }
}
