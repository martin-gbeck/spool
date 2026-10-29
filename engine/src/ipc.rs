use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Error, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::controls::{Controls, QueueView, SettingsChange, SettingsView, Status};

/// One JSON object per line, in and out. Anything that can drive the app from
/// the outside goes through here: the Python client, the desktop app and the CLI.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Speak {
        text: String,
        #[serde(default)]
        voice: Option<String>,
        #[serde(default)]
        instructions: Option<String>,
        /// When set, each chunk's audio and text land here as 001.mp3, 001.txt...
        #[serde(default)]
        save_dir: Option<String>,
        /// Shown in the app; the first words of the text when missing.
        #[serde(default)]
        title: Option<String>,
    },
    /// Queue audio files saved by an earlier speak, in the order given.
    PlayFiles {
        paths: Vec<String>,
        #[serde(default)]
        title: Option<String>,
    },
    Play,
    Pause,
    Toggle,
    /// Next chunk. `next` is the same thing.
    #[serde(alias = "next")]
    Skip,
    Previous,
    Restart,
    /// Relative with `by`, absolute with `to`, in seconds within the current chunk.
    Seek {
        #[serde(default)]
        by: Option<f64>,
        #[serde(default)]
        to: Option<f64>,
    },
    Jump { id: u64 },
    Remove { id: u64 },
    ClearPlayed,
    Stop,
    SetSpeed { speed: f32 },
    SetVolume { volume: f32 },
    Settings,
    SetSettings(SettingsChange),
    Status,
    Queue,
    Quit,
}

#[derive(Debug, Serialize)]
struct Reply {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue: Option<QueueView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<SettingsView>,
    #[serde(flatten)]
    status: Status,
}

impl Reply {
    fn ok(controls: &Controls) -> Self {
        Self {
            ok: true,
            error: None,
            chunks: None,
            queue: None,
            settings: None,
            status: controls.status(),
        }
    }

    fn spoken(chunks: usize, controls: &Controls) -> Self {
        Self {
            chunks: Some(chunks),
            ..Self::ok(controls)
        }
    }

    fn failed(message: String, controls: &Controls) -> Self {
        Self {
            ok: false,
            error: Some(message),
            ..Self::ok(controls)
        }
    }

    fn from_result(result: Result<(), Error>, controls: &Controls) -> Self {
        match result {
            Ok(()) => Self::ok(controls),
            Err(e) => Self::failed(e.to_string(), controls),
        }
    }

    fn found(found: bool, id: u64, controls: &Controls) -> Self {
        if found {
            Self::ok(controls)
        } else {
            Self::failed(format!("no chunk with id {id}"), controls)
        }
    }
}

/// Next to the config file, so everything this app owns lives in one directory.
pub fn socket_path() -> PathBuf {
    crate::config::config_path().with_file_name("rust_tts.sock")
}

/// Takes the lock that says this process is the app. One that is quitting
/// holds it for a moment after it stops answering, so this waits that out.
pub fn instance_lock() -> Result<File, Error> {
    let path = socket_path().with_file_name("rust_tts.lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => bail!("rust tts is already running"),
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// Client side. Sends one command to a running app and returns its raw reply.
pub fn send_blocking(command: &Command) -> Result<String, Error> {
    let stream = std::os::unix::net::UnixStream::connect(socket_path())
        .context("rust tts is not running (start it by running this binary with no arguments)")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut payload = serde_json::to_vec(command)?;
    payload.push(b'\n');

    let mut writer = &stream;
    writer.write_all(&payload)?;
    writer.flush()?;

    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;

    Ok(reply)
}

/// Server side. Runs for the life of the process alongside the event loop.
pub async fn serve(controls: Controls) -> Result<(), Error> {
    let path = socket_path();

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    // Only the holder of instance_lock() gets here, so a file here is a dead one.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let listener =
        UnixListener::bind(&path).with_context(|| format!("cannot bind {}", path.display()))?;

    loop {
        let (stream, _) = listener.accept().await?;
        let controls = controls.clone();

        tokio::spawn(async move {
            if let Err(e) = handle(stream, controls).await {
                eprintln!("ipc connection failed: {e}");
            }
        });
    }
}

async fn handle(stream: UnixStream, controls: Controls) -> Result<(), Error> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = AsyncBufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(command) => {
                // Every command except the app's polling, so the log shows who did what.
                if !matches!(command, Command::Status | Command::Queue | Command::Settings) {
                    eprintln!("command: {}", line.chars().take(160).collect::<String>());
                }
                apply(command, &controls)
            }
            Err(e) => Reply::failed(format!("bad command: {e}"), &controls),
        };

        let mut out = serde_json::to_vec(&reply)?;
        out.push(b'\n');
        writer.write_all(&out).await?;
        writer.flush().await?;
    }

    Ok(())
}

fn apply(command: Command, controls: &Controls) -> Reply {
    match command {
        Command::Speak {
            text,
            voice,
            instructions,
            save_dir,
            title,
        } => match controls.speak(&text, voice, instructions, save_dir.map(PathBuf::from), title) {
            Ok(chunks) => Reply::spoken(chunks, controls),
            Err(e) => Reply::failed(e.to_string(), controls),
        },
        Command::PlayFiles { paths, title } => {
            let count = controls.play_files(paths.into_iter().map(PathBuf::from).collect(), title);
            Reply::spoken(count, controls)
        }
        Command::Play => {
            controls.play();
            Reply::ok(controls)
        }
        Command::Pause => {
            controls.pause();
            Reply::ok(controls)
        }
        Command::Toggle => {
            controls.toggle();
            Reply::ok(controls)
        }
        Command::Skip => {
            controls.skip();
            Reply::ok(controls)
        }
        Command::Previous => {
            controls.previous();
            Reply::ok(controls)
        }
        Command::Restart => {
            controls.restart();
            Reply::ok(controls)
        }
        Command::Seek { by, to } => {
            match (by, to) {
                (_, Some(to)) => controls.seek_to(to),
                (Some(by), None) => controls.seek_by(by),
                (None, None) => return Reply::failed("seek needs by or to".into(), controls),
            }
            Reply::ok(controls)
        }
        Command::Jump { id } => Reply::found(controls.jump(id), id, controls),
        Command::Remove { id } => Reply::found(controls.remove(id), id, controls),
        Command::ClearPlayed => {
            controls.clear_played();
            Reply::ok(controls)
        }
        Command::Stop => {
            controls.stop();
            Reply::ok(controls)
        }
        Command::SetSpeed { speed } => Reply::from_result(controls.set_speed(speed), controls),
        Command::SetVolume { volume } => Reply::from_result(controls.set_volume(volume), controls),
        Command::Settings => Reply {
            settings: Some(controls.settings()),
            ..Reply::ok(controls)
        },
        Command::SetSettings(change) => {
            let mut reply = Reply::from_result(controls.change_settings(change), controls);
            reply.settings = Some(controls.settings());
            reply
        }
        Command::Status => Reply::ok(controls),
        Command::Queue => Reply {
            queue: Some(controls.queue()),
            ..Reply::ok(controls)
        },
        Command::Quit => {
            controls.stop();
            // Off the socket path before the reply goes out, so a call that
            // comes in during the exit delay starts a fresh app instead of
            // queueing audio on this dying one.
            let _ = std::fs::remove_file(socket_path());
            let reply = Reply::ok(controls);
            // Exit on a delay so this reply makes it out of the socket first.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                std::process::exit(0);
            });
            reply
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_from_the_json_the_clients_send() {
        let parse = |s: &str| serde_json::from_str::<Command>(s).unwrap();
        assert!(matches!(parse(r#"{"cmd":"next"}"#), Command::Skip));
        assert!(matches!(parse(r#"{"cmd":"seek","by":-10}"#), Command::Seek { by: Some(b), to: None } if b == -10.0));
        assert!(matches!(parse(r#"{"cmd":"jump","id":7}"#), Command::Jump { id: 7 }));
        assert!(matches!(parse(r#"{"cmd":"set_settings","voice":"ash"}"#), Command::SetSettings(c) if c.voice.as_deref() == Some("ash") && c.model.is_none()));
        assert!(matches!(parse(r#"{"cmd":"speak","text":"hi","save_dir":"/x"}"#), Command::Speak { title: None, .. }));
    }
}
