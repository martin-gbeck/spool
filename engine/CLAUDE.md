# tts

The Rust menu bar app behind `tts_speak`. Source here, binary at
`~/.local/bin/rust-tts`, Python client at `../tts.py`.

## Local rules

**Build with `./build.sh`, never a bare `cargo build`.** The script puts the
target directory at `~/.cache/rust-tts-target`, outside iCloud. A `target/` in
this folder means gigabytes of intermediate files syncing to iCloud. `Cargo.lock`
is committed on purpose: this is a binary, so the pinned versions are the point.

**A rebuild does not reach a running app.** Restart it: `rust-tts quit`, then
any call to `tts_speak` starts the new binary.

**The API key comes from the Python side.** The Rust config reads
`OPENAI_API_KEY`; `../tts.py` maps `TTS_OPENAI_API_KEY` out of the vault `.env`
into the app's environment when it spawns it. Launch the binary by hand and
that env var has to be set by hand too.

**Config lives at `~/Library/Application Support/rust_tts/`**, not here, and
holds his voice and model choices. The socket and the log sit beside it.

## Shape

`main.rs` is both the app and its own client: no arguments runs the menu bar
app, arguments send one command to a running one. `controls.rs` holds the
shared state the hotkey, the tray menu and the socket all act on. `ipc.rs` is
the socket protocol. The rest was already here.

The hotkey needs macOS Accessibility permission; the socket does not, so
missing permission logs a line and the app runs on without it. **Without that
permission it must not register the hotkey at all** (2026-10-16): it used to
claim Ctrl+Space anyway and do nothing, which killed the hotkey of his own
Desktop `rust_tts` build every time `tts_speak` started this app. He runs that
Desktop build (from `~/Documents/Programming/rust_tts`) for the hotkey.

**Never trust rodio's `stop()`, `clear()` or `len()` alone** (2026-10-25). rodio acts on stop and clear only when the device pulls audio, and cpal pauses a stream for good, with no error, when its device drops out (his AirBuds do this). That froze playback while status said speaking with chunks queued, and a stop did nothing. So `controls.rs` keeps each queued chunk's bytes, a watchdog reopens the output on the current default device when the playhead sits still for 3 seconds or the default device changes, and stop replaces the whole output. `speaking` in status means the playhead moved in the last second. One instance only, held by `rust_tts.lock`.

**Voice: calm, never dramatic.** Martin, 2026-10-02: the old `coral` voice with a cheerful prompt was unbearable. Default is now `onyx` (his pick) with a patient-teacher prompt: slow, calm, never excited. Never pass an energetic voice or an upbeat `instructions` string.

**Every reading is saved** (2026-10-21, his ask, to stop paying twice for the same audio). `tts.py` makes a folder per speak under `~/Library/Application Support/rust_tts/library/` (outside iCloud, about 1 MB of audio a minute) with `transcript.md`; the app writes `001.mp3` and `001.txt` per chunk into it. `tts_replay` plays a folder again through the `play_files` socket command, from a chunk number or the chunk holding a phrase. Replay before re-sending.
