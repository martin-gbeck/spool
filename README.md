# Spool

A macOS menu bar app that reads text aloud with OpenAI's text-to-speech. Select text anywhere,
press Ctrl+Space, and Spool reads it. Long texts are split into chunks and queued, so playback
starts after the first chunk instead of waiting for the whole thing.

Spool is two parts:

- **engine/**: a Rust daemon that owns the OpenAI connection, the queue and the audio output. It
  listens on a unix socket, so anything can feed it: the app, the `rust-tts` command line, or your
  own scripts.
- **app/**: a SwiftUI player on top of the engine, with the queue, a transcript that follows along,
  speed control and media keys.

## Requirements

- macOS 14 or later
- Rust (cargo) and Xcode's Swift toolchain
- An OpenAI API key

## Install

```sh
./build.sh
```

This builds both parts, puts `Spool.app` in `/Applications`, and links `~/.local/bin/rust-tts`.
It signs with the first Apple Development identity in your keychain, or ad-hoc if there is none
(then macOS asks for Accessibility again after every build). Set `SIGN_IDENTITY` to pick one.
`engine/.cargo/config.toml` sets the cargo build folder; change it to suit your machine.

The first run writes a config file to
`~/Library/Application Support/rust_tts/rust_tts_config.jsonc`. By default the key is read from
the `OPENAI_API_KEY` environment variable; the file also takes a `.env` file or the key itself.
Voice, model, speed and the shortcut are set there too.

Ctrl+Space needs Accessibility permission for Spool (System Settings, Privacy & Security).

## Command line

```sh
rust-tts speak "Hello there"     # or pipe text in: pbpaste | rust-tts speak
rust-tts pause | play | toggle | skip | previous | restart | stop
rust-tts seek 15                 # seconds, negative to go back
rust-tts speed 1.25
rust-tts queue | status
```

## Use with Claude

`integrations/claude/spool_mcp.py` is an MCP server that lets Claude read text aloud through
Spool, in Claude Code or Claude Desktop. It talks to the engine's socket, so what Claude queues
lands in the same queue as your Ctrl+Space, and you control it from the app as usual. If Spool
isn't running, the first call opens it. You need [uv](https://docs.astral.sh/uv/), which
installs the one dependency (the `mcp` Python SDK) on first run.

Claude Code (`--scope user` makes it available in every folder, not only the one you run it in):

```sh
claude mcp add --scope user spool -- uv run --script /path/to/spool/integrations/claude/spool_mcp.py
```

Claude Desktop: add this to `~/Library/Application Support/Claude/claude_desktop_config.json`
and restart the app. Use the full path to `uv` (`which uv` prints it), because Claude Desktop
doesn't load your shell's PATH.

```json
{
  "mcpServers": {
    "spool": {
      "command": "/full/path/to/uv",
      "args": ["run", "--script", "/path/to/spool/integrations/claude/spool_mcp.py"]
    }
  }
}
```

The tools:

- `spool_speak` reads text aloud and saves the audio and transcript to the library.
- `spool_replay` plays a saved reading again, from the start, a chunk number or a phrase, at no cost.
- `spool_library` lists saved readings, newest first, optionally only those containing a word.
- `spool_control` pauses, resumes, skips, seeks, stops, sets speed or volume, or reports status.
- `spool_queue` shows what is playing, what played and what is up next.
- `spool_arrange` moves, jumps to or removes a chunk in the queue.

OpenAI charges per character for every `spool_speak`, and Claude can send a lot of text at
once. Replays are free. The server sets no limit. If you want one, such as Claude asking
before reading anything over 150 words, put it in your own instructions to Claude (your
`CLAUDE.md`, for example).

Optional environment variables: `SPOOL_LIBRARY_DIR` sets where Claude's readings are saved
(default `~/Library/Application Support/rust_tts/library`, where the app saves its own; set it
elsewhere and `spool_library` no longer sees the app's readings), `SPOOL_SOCKET` where to find
the engine's socket (it doesn't move the socket), and `SPOOL_APP` the app to open.

The tests run against a fake socket, with no audio and no OpenAI calls:

```sh
cd integrations/claude
uv run --with "mcp>=2.3,<3" --with pytest pytest -q
```

## License

MIT
