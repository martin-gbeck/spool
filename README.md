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

## License

MIT
