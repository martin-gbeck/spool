import ServiceManagement
import SwiftUI

/// Voice and playback settings. Everything here is stored by the engine in its
/// config file, so Claude's tts_speak calls use the same voice he picks here.
struct SettingsView: View {
    @Environment(PlayerModel.self) private var model
    @State private var instructions = ""
    @State private var opensAtLogin = SMAppService.mainApp.status == .enabled
    @State private var loginError: String?

    var body: some View {
        Form {
            if let settings = model.settings {
                Section("Voice") {
                    HStack {
                        Picker("Voice", selection: Binding(
                            get: { settings.voice },
                            set: { model.change(voice: $0) }
                        )) {
                            ForEach(settings.voices, id: \.self) { Text($0.capitalized).tag($0) }
                        }
                        Button("Preview") { model.preview(voice: settings.voice) }
                            .help("Speaks one sample sentence in this voice. Costs one short API call.")
                    }

                    Picker("Model", selection: Binding(
                        get: { settings.model },
                        set: { model.change(model: $0) }
                    )) {
                        ForEach(settings.models, id: \.self) { Text($0).tag($0) }
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        Text("Tone prompt")
                        TextEditor(text: $instructions)
                            .font(.body)
                            .frame(minHeight: 80)
                            .scrollContentBackground(.hidden)
                            .padding(4)
                            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
                        HStack {
                            Text(settings.model == "gpt-4o-mini-tts"
                                 ? "How the voice should sound. Applies to new text, not audio already fetched."
                                 : "Only gpt-4o-mini-tts follows a tone prompt.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Button("Save") { model.change(instructions: instructions) }
                                .disabled(instructions == settings.instructions)
                        }
                    }
                    .disabled(settings.model != "gpt-4o-mini-tts")
                }

                Section("Playback") {
                    LabeledContent("Speed") {
                        HStack {
                            Slider(
                                value: Binding(get: { model.speed }, set: { model.setSpeed(($0 * 20).rounded() / 20) }),
                                in: Format.minSpeed...Format.maxSpeed
                            )
                            Text(Format.speed(model.speed))
                                .monospacedDigit()
                                .frame(width: 48, alignment: .trailing)
                        }
                    }
                    LabeledContent("Volume") {
                        VolumeControl()
                    }
                    LabeledContent("Shortcut") {
                        HStack {
                            Text(settings.shortcut)
                            Button("Edit Config File") {
                                NSWorkspace.shared.open(
                                    [Engine.directory.appending(path: "rust_tts_config.jsonc")],
                                    withApplicationAt: URL(filePath: "/System/Applications/TextEdit.app"),
                                    configuration: .init()
                                )
                            }
                        }
                    }
                    Text("Speed and volume apply at once, also to chunks already queued. A new shortcut needs Restart Engine.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Section("App") {
                    Toggle("Open at login", isOn: $opensAtLogin)
                        .onChange(of: opensAtLogin) { _, on in setLoginItem(on) }
                    if let loginError {
                        Text(loginError).font(.caption).foregroundStyle(.red)
                    }
                    HStack {
                        Button("Restart Engine") { model.restartEngine() }
                        Button("Open Engine Log") { NSWorkspace.shared.open(Engine.logURL) }
                    }
                }
            } else {
                Text(model.engineProblem ?? "Connecting to the engine…")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 500)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear { instructions = model.settings?.instructions ?? "" }
        .onChange(of: model.settings?.instructions) { _, new in instructions = new ?? "" }
    }

    private func setLoginItem(_ on: Bool) {
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            loginError = nil
        } catch {
            loginError = error.localizedDescription
            opensAtLogin = SMAppService.mainApp.status == .enabled
        }
    }
}
