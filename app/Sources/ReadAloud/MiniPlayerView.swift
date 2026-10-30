import SwiftUI

/// The menu bar dropdown: what is playing, the transport, and a way into the full window.
struct MiniPlayerView: View {
    @Environment(PlayerModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @FocusState private var typing: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let entry = model.current {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(entry.title)  ·  \(entry.index) of \(entry.of)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Text(Format.preview(entry.text, limit: 110))
                        .lineLimit(2)
                }
            } else {
                Text(model.engineProblem ?? "Nothing playing")
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
            }

            Scrubber()

            HStack {
                TransportButtons(size: .small)
                Spacer()
                SpeedMenu()
            }

            if let next = model.queue.upNext.first {
                Text("Next: \(Format.preview(next.text, limit: 70))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            PasteField(focus: $typing)

            Divider()

            HStack {
                Button("Open Read Aloud") {
                    openWindow(id: "main")
                    NSApp.activate()
                }
                Spacer()
                Menu {
                    Button("Add Audio…") { model.chooseAudio() }
                    SettingsLink { Text("Settings…") }
                    Button("Restart Engine") { model.restartEngine() }
                    Divider()
                    Button("Quit Window, Keep Engine") { NSApp.terminate(nil) }
                    Button("Quit Everything") {
                        model.quitEngine()
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { NSApp.terminate(nil) }
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
            }
        }
        .padding(14)
        .frame(width: 340)
        .dropDestination(for: URL.self) { urls, _ in model.add(files: urls) }
    }
}
