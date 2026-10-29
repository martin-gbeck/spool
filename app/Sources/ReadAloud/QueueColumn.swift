import SwiftUI

/// One of the two lists. Up next runs in play order; played runs most recent
/// first, so the chunk that just finished is at the top.
struct QueueColumn: View {
    enum Kind { case upNext, played }

    @Environment(PlayerModel.self) private var model
    let kind: Kind

    private var entries: [Entry] {
        kind == .upNext ? model.queue.upNext : model.queue.played
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(kind == .upNext ? "Up next" : "Played")
                    .font(.headline)
                Text("\(entries.count)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer()
                Button(kind == .upNext ? "Clear" : "Clear history") {
                    kind == .upNext ? model.stop() : model.clearPlayed()
                }
                .buttonStyle(.borderless)
                .font(.caption)
                .disabled(entries.isEmpty)
                .help(kind == .upNext
                      ? "Drop everything up next. The chunk playing stops and moves to played."
                      : "Forget the played chunks.")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)

            if entries.isEmpty {
                Text(kind == .upNext ? "Nothing queued" : "Nothing played yet")
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(entries) { entry in
                    QueueRow(entry: entry, showTitle: showsTitle(entry))
                }
                .listStyle(.inset)
                .scrollContentBackground(.hidden)
            }
        }
        .frame(minWidth: 260, maxWidth: .infinity)
    }

    /// The reading's title goes on the first row of each reading, not on every row.
    private func showsTitle(_ entry: Entry) -> Bool {
        guard let index = entries.firstIndex(of: entry), index > 0 else { return true }
        return entries[index - 1].reading != entry.reading
    }
}

private struct QueueRow: View {
    @Environment(PlayerModel.self) private var model
    let entry: Entry
    let showTitle: Bool
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(entry.index)/\(entry.of)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 42, alignment: .trailing)
                .padding(.top, 1)

            VStack(alignment: .leading, spacing: 2) {
                if showTitle {
                    Text(entry.title)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Text(Format.preview(entry.text))
                    .lineLimit(2)
                    .foregroundStyle(entry.state == .failed ? .secondary : .primary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            stateBadge
            if hovering {
                Button { model.jump(to: entry) } label: { Image(systemName: "play.fill") }
                    .help("Play from here")
                Button { model.remove(entry) } label: { Image(systemName: "xmark") }
                    .help("Remove")
            }
        }
        .buttonStyle(.borderless)
        .padding(.vertical, 3)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { model.jump(to: entry) }
        .contextMenu {
            Button("Play from Here") { model.jump(to: entry) }
            Button("Remove") { model.remove(entry) }
            Divider()
            Button("Copy Text") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(entry.text, forType: .string)
            }
        }
    }

    @ViewBuilder private var stateBadge: some View {
        switch entry.state {
        case .loading:
            ProgressView().controlSize(.mini).help("Fetching audio")
        case .failed:
            Image(systemName: "exclamationmark.circle")
                .foregroundStyle(.orange)
                .help("The audio failed to load. It will be skipped.")
        case .ready:
            EmptyView()
        }
    }
}
