import SwiftUI

struct NowPlaying: View {
    @Environment(PlayerModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text(heading)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(model.current.map { Format.preview($0.text, limit: 160) } ?? "Nothing playing")
                    .font(.title3.weight(.medium))
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, minHeight: 50, alignment: .topLeading)
            }

            Scrubber()

            HStack(spacing: 18) {
                TransportButtons(size: .large)
                Spacer()
                VolumeControl()
                SpeedMenu()
            }
        }
    }

    private var heading: String {
        guard let entry = model.current else {
            let waiting = model.status?.loading ?? 0
            return waiting > 0 ? "Loading \(waiting) chunk\(waiting == 1 ? "" : "s")…" : "Queue is empty"
        }
        var parts = ["Chunk \(entry.index) of \(entry.of)", entry.title]
        if model.status?.stalled == true { parts.append("waiting for audio") }
        if entry.state == .loading { parts.append("loading") }
        return parts.joined(separator: "  ·  ")
    }
}

/// Position in the current chunk. Dragging holds the knob locally and seeks
/// on release, the way a video player does.
struct Scrubber: View {
    @Environment(PlayerModel.self) private var model

    var body: some View {
        @Bindable var model = model
        let duration = max(model.status?.duration ?? 0, 0.1)
        let position = min(model.scrubbing ?? model.status?.position ?? 0, duration)

        VStack(spacing: 2) {
            Slider(
                value: Binding(get: { position }, set: { model.scrubbing = $0 }),
                in: 0...duration,
                onEditingChanged: { editing in
                    if !editing, let target = model.scrubbing { model.seek(to: target) }
                }
            )
            .controlSize(.small)
            .disabled(model.status?.duration == nil)

            HStack {
                Text(Format.time(position))
                Spacer()
                Text("-" + Format.time(duration - position))
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
    }
}

struct TransportButtons: View {
    enum Size { case large, small }

    @Environment(PlayerModel.self) private var model
    let size: Size

    var body: some View {
        HStack(spacing: size == .large ? 16 : 10) {
            button("backward.end.fill", help: "Previous chunk (⇧P)") { model.previous() }
            button("gobackward.10", help: "Back 10 seconds (J)") { model.seek(by: -10) }
            button(model.isPlaying ? "pause.circle.fill" : "play.circle.fill",
                   help: "Play or pause (Space)", scale: 1.6) { model.togglePlay() }
            button("goforward.10", help: "Forward 10 seconds (L)") { model.seek(by: 10) }
            button("forward.end.fill", help: "Next chunk (⇧N)") { model.next() }
            button("arrow.counterclockwise", help: "Restart this chunk (0)") { model.restart() }
        }
        .buttonStyle(.borderless)
    }

    private func button(
        _ symbol: String, help: String, scale: CGFloat = 1, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: (size == .large ? 18 : 14) * scale))
                .frame(minWidth: 24, minHeight: 24)
                .contentShape(Rectangle())
        }
        .help(help)
    }
}

struct SpeedMenu: View {
    @Environment(PlayerModel.self) private var model

    var body: some View {
        Menu {
            ForEach(Format.speeds, id: \.self) { speed in
                Button {
                    model.setSpeed(speed)
                } label: {
                    if abs(speed - model.speed) < 0.01 {
                        Label(Format.speed(speed), systemImage: "checkmark")
                    } else {
                        Text(Format.speed(speed))
                    }
                }
            }
            Divider()
            Button("Faster (⇧.)") { model.faster() }
            Button("Slower (⇧,)") { model.slower() }
        } label: {
            Text(Format.speed(model.speed))
                .monospacedDigit()
        }
        .menuStyle(.borderedButton)
        .fixedSize()
        .help("Playback speed. The pitch stays the same.")
    }
}

struct VolumeControl: View {
    @Environment(PlayerModel.self) private var model
    @State private var dragging: Double?

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "speaker.fill")
                .foregroundStyle(.secondary)
            Slider(
                value: Binding(
                    get: { dragging ?? model.status?.volume ?? 1 },
                    set: { dragging = $0; model.setVolume($0) }
                ),
                in: 0...1,
                onEditingChanged: { editing in if !editing { dragging = nil } }
            )
            .controlSize(.small)
            .frame(width: 90)
        }
        .help("Volume of Read Aloud only")
    }
}

/// A field to paste text into. Return queues it behind whatever is playing,
/// the same as Ctrl+Space; Option+Return adds a line break.
struct PasteField: View {
    @Environment(PlayerModel.self) private var model
    @State private var text = ""
    var focus: FocusState<Bool>.Binding

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Paste text to read…", text: $text, axis: .vertical)
                .lineLimit(1...4)
                .textFieldStyle(.roundedBorder)
                .focused(focus)
                .onSubmit(add)
            Button("Add", action: add)
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .help("Add to the queue (Return)")
        }
    }

    private func add() {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        model.speak(trimmed)
        text = ""
    }
}
