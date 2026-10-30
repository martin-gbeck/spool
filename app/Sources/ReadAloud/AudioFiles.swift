import Foundation
import UniformTypeIdentifiers

/// Audio he drops on the window or picks with Add Audio, sorted into the
/// readings to queue. A dropped folder brings its audio files in name order,
/// which is chunk order for a library reading (001.mp3, 002.mp3, ...).
enum AudioFiles {
    static let extensions: Set<String> = ["mp3", "wav", "flac", "ogg"]
    static let types: [UTType] = [.mp3, .wav, .audio, .folder]

    struct Reading: Equatable {
        let paths: [String]
        /// Nil lets the engine read the title from the folder's transcript.md.
        let title: String?
    }

    /// One reading per folder, in the order they were dropped.
    static func readings(from urls: [URL]) -> [Reading] {
        var order: [URL] = []
        var files: [URL: [URL]] = [:]
        for url in urls.filter(\.isFileURL) {
            for file in audio(in: url) {
                let folder = file.deletingLastPathComponent()
                if files[folder] == nil { order.append(folder) }
                files[folder, default: []].append(file)
            }
        }
        return order.map { folder in
            let group = files[folder] ?? []
            return Reading(paths: group.map(\.path), title: title(folder: folder, files: group))
        }
    }

    private static func audio(in url: URL) -> [URL] {
        var isFolder: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isFolder) else { return [] }
        guard isFolder.boolValue else {
            return extensions.contains(url.pathExtension.lowercased()) ? [url] : []
        }
        let contents = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) ?? []
        return contents
            .filter { extensions.contains($0.pathExtension.lowercased()) }
            .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
    }

    private static func title(folder: URL, files: [URL]) -> String? {
        if FileManager.default.fileExists(atPath: folder.appendingPathComponent("transcript.md").path) {
            return nil
        }
        if files.count == 1 { return files[0].deletingPathExtension().lastPathComponent }
        return folder.lastPathComponent
    }
}
