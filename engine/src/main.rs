mod audio;
mod chunking;
mod clipboard;
mod config;
mod controls;
mod deck;
mod ipc;
mod tts;

use std::io::Read;

use anyhow::{Error, bail};

use arboard::Clipboard;

use tao::event::Event;

use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager,
    HotKeyState::{self},
    hotkey::{Code, HotKey, Modifiers},
};

use enigo::Enigo;

use crate::clipboard::CurrentClipboard;
use crate::controls::Controls;
use crate::deck::Place;
use crate::ipc::Command;

const USAGE: &str = "\
rust tts, the engine behind Spool

  rust-tts                       run the engine (hotkey + socket, no window)
  rust-tts speak [text]          speak text, or stdin when no text is given
  rust-tts pause|play|toggle     pause, resume, or flip whichever it is
  rust-tts next|previous|restart move between chunks or start this one over
  rust-tts seek <seconds>        jump within the chunk, negative goes back
  rust-tts speed <x>             playback speed, 0.5 to 3, pitch kept
  rust-tts stop                  drop everything up next
  rust-tts status|queue|quit
";

enum AppEvent {
    Hotkey(GlobalHotKeyEvent),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // With arguments this is a client: hand the command to the running app and
    // print what it says back. Without them, it becomes that running app.
    if let Some(command) = parse_command()? {
        print!("{}", ipc::send_blocking(&command)?);
        return Ok(());
    }

    // Held until the process exits. Two copies starting at once (two tool
    // calls racing to launch it) would otherwise both bind the socket, and the
    // loser would keep the audio device with nobody able to reach it.
    let _instance = ipc::instance_lock()?;

    // Load (or create on first run) the config file; resolves the api key too.
    // Arc because every spawned fetch task needs its own owned handle to it.
    let config = config::load()?;

    // Make loop
    let mut event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    // No Dock icon of its own: the Spool app is the face, this is the engine.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    let hk_proxy = event_loop.create_proxy();
    // Set event handler for hotkey events
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = hk_proxy.send_event(AppEvent::Hotkey(e));
    }));

    // Hotkey from config if set ("ctrl+space" style string), else Ctrl+Space.
    // HotKey implements FromStr, so .parse() works; ? surfaces a bad string.
    let hotkey = match &config.shortcut {
        Some(s) => s.parse::<HotKey>()?,
        None => HotKey::new(Some(Modifiers::CONTROL), Code::Space),
    };

    // Both of these need macOS Accessibility permission, and neither is worth
    // refusing to start over: without them the hotkey is dead but the socket,
    // and the app driving it still work.
    let mut write_conn = match Enigo::new(&enigo::Settings::default()) {
        Ok(enigo) => Some(enigo),
        Err(e) => {
            eprintln!(
                "cannot simulate input, so the hotkey is left free for other apps: {e}\n\
                 grant Accessibility to rust-tts in System Settings > Privacy & Security"
            );
            None
        }
    };

    // Without input permission the hotkey can do nothing, and registering it
    // anyway steals the key from any other app bound to it. That silently broke
    // the Desktop rust_tts build's Ctrl+Space every time tts_speak started this
    // app (2026-10-16).
    let _manager = match write_conn {
        None => None,
        Some(_) => match GlobalHotKeyManager::new() {
            Ok(manager) => match manager.register(hotkey) {
                Ok(()) => Some(manager),
                Err(e) => {
                    eprintln!("hotkey not registered: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!("hotkey manager unavailable: {e}");
                None
            }
        },
    };

    let controls = Controls::new(config)?;

    // The event loop below owns the main thread from here on, so the socket
    // server has to live on the runtime's own threads.
    let server_controls = controls.clone();
    tokio::spawn(async move {
        if let Err(e) = ipc::serve(server_controls).await {
            eprintln!("socket server stopped: {e}");
        }
    });

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::UserEvent(AppEvent::Hotkey(hotkey_event)) = event
            && hotkey_event.state == HotKeyState::Pressed
            && let Some(write_conn) = write_conn.as_mut()
            && let Err(e) = speak_selection(write_conn, &controls)
        {
            eprintln!("{e}");
        }
    });
}

/// None means "no arguments, run as the app". Anything unrecognised is an
/// error rather than a silent daemon start, which is easier to debug.
fn parse_command() -> Result<Option<Command>, Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let Some(verb) = args.first() else {
        return Ok(None);
    };

    let command = match verb.as_str() {
        "speak" => {
            let text = if args.len() > 1 {
                args[1..].join(" ")
            } else {
                let mut stdin = String::new();
                std::io::stdin().read_to_string(&mut stdin)?;
                stdin
            };

            Command::Speak {
                text,
                voice: None,
                instructions: None,
                save_dir: None,
                title: None,
                at: Place::Last,
            }
        }
        "play" | "resume" => Command::Play,
        "pause" => Command::Pause,
        "toggle" => Command::Toggle,
        "skip" | "next" => Command::Skip,
        "previous" | "prev" => Command::Previous,
        "restart" => Command::Restart,
        "seek" => Command::Seek {
            by: Some(number(&args, "seek")?),
            to: None,
        },
        "speed" => Command::SetSpeed {
            speed: number(&args, "speed")? as f32,
        },
        "queue" => Command::Queue,
        "stop" => Command::Stop,
        "status" => Command::Status,
        "quit" => Command::Quit,
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            std::process::exit(0);
        }
        other => bail!("unknown command {other:?}\n\n{USAGE}"),
    };

    Ok(Some(command))
}

fn number(args: &[String], verb: &str) -> Result<f64, Error> {
    match args.get(1).map(|a| a.parse::<f64>()) {
        Some(Ok(n)) => Ok(n),
        _ => bail!("{verb} needs a number\n\n{USAGE}"),
    }
}

/// The hotkey path: copy whatever is selected, speak it, put the clipboard back
/// the way it was. Falls back to the existing clipboard if nothing is selected.
fn speak_selection(write_conn: &mut Enigo, controls: &Controls) -> Result<(), Error> {
    let mut clipboard = Clipboard::new()?;

    let current_clipboard = clipboard::get_current_clipboard(&mut clipboard);

    let selection = clipboard::get_text_from_selection(write_conn, &mut clipboard);

    let clip_text = match current_clipboard {
        CurrentClipboard::Text(ref t) => t,
        _ => "",
    }
    .to_string();

    clipboard::set_clipboard(current_clipboard, &mut clipboard)?;

    let mut text = selection?;

    if text.is_empty() {
        if clip_text.is_empty() {
            return Ok(());
        }

        text = clip_text;
    }

    controls.speak(&text, None, None, None, None, Place::Last)?;

    Ok(())
}
