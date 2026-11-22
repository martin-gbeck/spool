# Spool backlog

## How it works

Every change Martin asks for is filed here the day he sends it, numbered once, never renumbered.
`[ ]` to do, `[~]` being built (session, branch, date). Built items are cut from here and get a row in
`docs/done.md`; anything only he can check gets a line in `docs/to-test.md`. Builds happen in their
own sessions on a branch; the main session merges and pushes on his go.

## Settings and the API key

Files: `app/Sources/ReadAloud/SettingsView.swift`, `app/Sources/ReadAloud/Engine.swift` (launches the
engine), `engine/src/config.rs` (key methods). Also the vault client
`~/Library/Mobile Documents/iCloud~md~obsidian/Documents/SecondBrain/system/tools/tts.py`.

- [ ] **1. Spool keeps its own OpenAI API key, entered in the app** (his ask, 2026-11-02)
  - Why: one key per app, so OpenAI's usage page shows what Spool spends on its own. On 2026-11-02
    he had $2.50 of gpt-6.1-sol spend no app accounted for, and a shared key made it untraceable.
  - Now: the engine's config (`~/Library/Application Support/rust_tts/rust_tts_config.jsonc`) has
    `"api_key_method": "env_file"`, reading `TTS_OPENAI_API_KEY` from the vault `.env`. `config.rs`
    already supports `sys_var`, `config_var` and `env_file`. `tts.py` (`_start`, about line 184)
    raises if `TTS_OPENAI_API_KEY` is missing when it launches the bare binary.
  - Change: a "OpenAI API key" field in Settings, a SecureField with Save, storing the key in the
    macOS Keychain. The app passes it to the engine when it launches it (as `OPENAI_API_KEY` with a
    method that reads it, or a new `keychain` method in `config.rs`); it is never written to the
    config file in plain text. Settings shows whether a key is set (last 4 characters only) and a
    clear error from the engine when the key is rejected. `tts.py` stops needing
    `TTS_OPENAI_API_KEY` and the vault `.env` line can go.
  - Done when: with the vault `.env` line removed, a key entered in Settings survives a restart,
    `tts.py speak` reads aloud, and a wrong key shows the rejection in Settings.

## Voice

- [~] **2. Spool reads through Speechify, voice Benjamin, model simba-3.2** (his call, 2026-11-22; branch `speechify`)
  - Why: Benjamin on Speechify is Q's and M's voice (samples in the vault `output/tech/speechify-voices/`).
    OpenAI `onyx` stays as a fallback in Settings.
  - API: `POST https://api.sws.speechify.com/v1/audio/speech`, header `Authorization: Bearer <key>`, JSON
    `{"input": text, "voice_id": "benjamin", "model": "simba-3.2", "audio_format": "mp3"}`; the reply is JSON
    whose `audio_data` is the mp3 in base64. Input is chunked to 1800 bytes per request.
  - Key: `SPEECHIFY_API_KEY` in the vault `.env`, read through the same `api_key_method` and `api_key_file`
    as the OpenAI key (`speechify_api_key_var_name` in the config).
  - Config: `engine` ("speechify" default, "openai" fallback), `speechify_voice`, `speechify_model`. Files
    from before the switch move to Speechify by themselves and keep `voice`/`model` as the OpenAI settings.
  - Settings: an Engine picker; voice and model follow the engine; the tone prompt shows for OpenAI only.
  - Done when: Ctrl+Space on text speaks as Benjamin, and Engine = OpenAI in Settings speaks as before.
