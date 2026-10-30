import SwiftUI

/// The main window: now playing and the two lists on the left, the transcript
/// of the reading on the right.
struct PlayerView: View {
    @Environment(PlayerModel.self) private var model
    @FocusState private var focused: Bool
    @FocusState private var typing: Bool

    var body: some View {
        HSplitView {
            VStack(spacing: 0) {
                Banners()
                NowPlaying()
                    .padding(20)
                PasteField(focus: $typing)
                    .padding(.horizontal, 20)
                    .padding(.bottom, 16)
                Divider()
                HStack(spacing: 0) {
                    QueueColumn(kind: .upNext)
                    Divider()
                    QueueColumn(kind: .played)
                }
            }
            .frame(minWidth: 560, maxWidth: .infinity)
            .background(.windowBackground)

            TranscriptView()
                .frame(minWidth: 280, idealWidth: 400, maxWidth: .infinity)
        }
        .navigationTitle(model.current?.title ?? "Spool")
        // Audio files or reading folders dropped anywhere on the window go to the end of the queue.
        .dropDestination(for: URL.self) { urls, _ in model.add(files: urls) }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear {
            focused = true
            NSApp.setActivationPolicy(.regular)
        }
        // Back to a menu bar app when the window closes, so no empty Dock icon is left.
        .onDisappear { NSApp.setActivationPolicy(.accessory) }
        .onKeyPress(action: handleKey)
    }

    /// YouTube's keys: space and K pause, J and L jump 10 seconds, the arrows
    /// 5, Shift with . and , change speed.
    private func handleKey(_ press: KeyPress) -> KeyPress.Result {
        // Typing in the paste field must not pause or seek.
        if typing { return .ignored }
        switch (press.key, press.characters) {
        case (.space, _), (_, "k"): model.togglePlay()
        case (_, "j"): model.seek(by: -10)
        case (_, "l"): model.seek(by: 10)
        case (.leftArrow, _): model.seek(by: -5)
        case (.rightArrow, _): model.seek(by: 5)
        case (_, ">"): model.faster()
        case (_, "<"): model.slower()
        case (_, "N"): model.next()
        case (_, "P"): model.previous()
        case (_, "0"): model.restart()
        default: return .ignored
        }
        return .handled
    }
}

/// Shown only when something needs him: the engine is down, or Ctrl+Space
/// has no permission yet.
private struct Banners: View {
    @Environment(PlayerModel.self) private var model

    var body: some View {
        if let problem = model.engineProblem {
            banner(problem, symbol: "exclamationmark.triangle.fill", tint: .orange) {
                Button("Open Log") { NSWorkspace.shared.open(Engine.logURL) }
            }
        } else if !model.hotkeyAllowed {
            banner(
                "Ctrl+Space needs Accessibility for Spool. Turn it on, then restart the engine.",
                symbol: "keyboard", tint: .blue
            ) {
                Button("Open Settings") { model.openAccessibilitySettings() }
                Button("Restart Engine") { model.restartEngine() }
            }
        }
    }

    private func banner(
        _ text: String, symbol: String, tint: Color, @ViewBuilder actions: () -> some View
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol).foregroundStyle(tint)
            Text(text)
                .font(.callout)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            actions()
        }
        .padding(12)
        .background(tint.opacity(0.12))
    }
}
