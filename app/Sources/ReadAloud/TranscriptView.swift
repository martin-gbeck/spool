import SwiftUI

/// The whole reading that is playing, chunk by chunk, with the chunk being
/// spoken highlighted and kept in view. OpenAI's speech API returns no word
/// timings, so the highlight follows chunks, not words. Double-clicking a
/// chunk plays from there. No text selection, since selection takes the double
/// click for itself; Copy is in the right-click menu.
struct TranscriptView: View {
    @Environment(PlayerModel.self) private var model

    var body: some View {
        let chunks = model.queue.transcript
        let currentID = model.current?.id

        VStack(alignment: .leading, spacing: 0) {
            Text(chunks.first?.title ?? "Transcript")
                .font(.headline)
                .lineLimit(2)
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            Divider()

            if chunks.isEmpty {
                Text("The text of what is playing shows here.")
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 4) {
                            ForEach(chunks) { chunk in
                                paragraph(chunk, isCurrent: chunk.id == currentID)
                                    .id(chunk.id)
                            }
                        }
                        .padding(12)
                    }
                    .onChange(of: currentID, initial: true) { _, id in
                        guard let id else { return }
                        withAnimation(.easeInOut(duration: 0.3)) {
                            proxy.scrollTo(id, anchor: .top)
                        }
                    }
                }
            }
        }
        .background(.background)
    }

    private func paragraph(_ chunk: Entry, isCurrent: Bool) -> some View {
        Text(chunk.text.trimmingCharacters(in: .whitespacesAndNewlines))
            .font(.body)
            .lineSpacing(4)
            .foregroundStyle(isCurrent ? .primary : .secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(isCurrent ? Color.accentColor.opacity(0.14) : .clear)
            )
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { model.jump(to: chunk) }
            .contextMenu {
                Button("Play from Here") { model.jump(to: chunk) }
                Button("Copy Text") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(chunk.text, forType: .string)
                }
            }
            .help("Double-click to play from here")
    }
}
