use tray_icon::menu::MenuEvent;

use crate::controls::Controls;

pub fn menu_controls(event: MenuEvent, controls: &Controls) {
    match event.id.as_ref() {
        "play" => controls.play(),
        "pause" => controls.pause(),
        "skip" => controls.skip(),
        "stop" => controls.stop(),
        "open_config" => {
            // `open <path>` on macOS opens the file in its default editor
            // -t = open as text in the default text editor; plain `open` fails
            // because no app claims the .jsonc extension
            let _ = std::process::Command::new("open")
                .arg("-t")
                .arg(crate::config::config_path())
                .spawn();
        }
        "quit" => std::process::exit(0),
        _ => {}
    }
}
