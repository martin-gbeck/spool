# Read Aloud

A SwiftUI app (`app/`) on top of a Rust engine (`engine/`). The engine owns the
OpenAI connection, the queue, the audio output, Ctrl+Space and a unix socket;
the app is one client of that socket, Claude's `tts_speak` (the vault's
`system/tools/tts.py`) is another, and the `rust-tts` CLI a third. They all see
and steer the same queue.

## Local rules

**Build with `./build.sh`, never a bare `cargo build` or `swift build` to
install.** It builds both, bundles the engine inside `/Applications/Read Aloud.app`, signs both with his Apple Development identity and links
`~/.local/bin/rust-tts` to the bundled engine. Build output lives in
`~/.cache` (`engine/.cargo/config.toml` sets the cargo target dir), outside
anything iCloud syncs. `Cargo.lock` is committed on purpose.

**The app starts the engine, so macOS makes Read Aloud responsible for it.**
That is why the Accessibility grant Ctrl+Space needs is on Read Aloud, and why
the signing identity must stay stable: an ad-hoc signature changes every build
and macOS forgets the grant. `tts.py` opens the app (`open -g -j`) rather than
the bare binary for the same reason.

**A menu bar app ignores AppleScript's quit.** `build.sh` stops the running app
with `pkill`. A stale copy left running keeps polling and sending commands,
which looked like the engine unpausing itself (2026-10-29).

**The API key comes from the vault `.env`.** The config's `api_key_method` is
`env_file`, pointing at `TTS_OPENAI_API_KEY` in the vault `.env`, so the
engine starts on its own without the Python side handing it the key.

**Config lives at `~/Library/Application Support/rust_tts/`**, not here, and
holds his voice, model, tone prompt, speed and volume. The app's settings write
it through the socket (`set_settings`, `set_speed`, `set_volume`); the engine
renders it from a template so the comments survive. The socket, the log and the
saved readings library sit beside it. The log records every command except the
app's polling.

## Shape

**The queue survives restarts** (2026-10-29, his ask). The engine writes it to
`queue.json` beside the config whenever the list changes and every 2 seconds
while playing, and restores it paused. That works because every reading is
saved to the library now, the hotkey's and the paste field's too: the deck
keeps file paths, not audio in memory.

`deck.rs` is the queue: one list with a cursor, played before it, playing at
it, up next after it. Restart, seek, previous, next, jump and remove are moves
of that cursor, and all of it is unit tested with no audio device. `audio.rs`
holds the one endless source each output plays: it pulls blocks from the deck,
so chunks run on with no gap, and runs Signalsmith Stretch for speeds other
than 1x, which keeps the pitch. Decoded audio is kept only for the chunks
around the cursor; every chunk keeps its mp3 for going back. `controls.rs`
ties deck, output, watchdog and config together; `ipc.rs` is the socket
protocol. The app polls `status` four times a second and refetches `queue`
only when `revision` changes.

Set `READ_ALOUD_SNAPSHOT=/path.png` and run the app binary to get a PNG of the
main window after 4 seconds: how a UI change gets checked without screen
recording permission.

The hotkey needs macOS Accessibility permission; the socket does not, so
missing permission logs a line and the app runs on without it. **Without that
permission it must not register the hotkey at all** (2026-10-16): it used to
claim Ctrl+Space anyway and do nothing, which killed the hotkey of his own
Desktop `rust_tts` build every time `tts_speak` started this app. Read Aloud
replaces that Desktop build (`~/Documents/Programming/rust_tts`); the two must
not run at once or they fight over Ctrl+Space.

**Never trust rodio's `stop()`, `clear()` or `len()` alone** (2026-10-25). rodio acts on stop and clear only when the device pulls audio, and cpal pauses a stream for good, with no error, when its device drops out (his AirBuds do this). That froze playback while status said speaking with chunks queued, and a stop did nothing. So the watchdog in `controls.rs` reopens the output on the current default device when the output stops pulling audio for 3 seconds while it should be playing, rewinding 2 seconds, or when the default device changes. The old output is dropped before the new one opens, so two never pull from the deck at once. One instance only, held by `rust_tts.lock`.

**Voice: calm, never dramatic.** Martin, 2026-10-02: the old `coral` voice with a cheerful prompt was unbearable. Default is now `onyx` (his pick) with a patient-teacher prompt: slow, calm, never excited. Never pass an energetic voice or an upbeat `instructions` string.

**Every reading is saved** (2026-10-21, his ask, to stop paying twice for the same audio). `tts.py` makes a folder per speak under `~/Library/Application Support/rust_tts/library/` (outside iCloud, about 1 MB of audio a minute) with `transcript.md`; the app writes `001.mp3` and `001.txt` per chunk into it. `tts_replay` plays a folder again through the `play_files` socket command, from a chunk number or the chunk holding a phrase. Replay before re-sending.
