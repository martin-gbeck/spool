use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard};

use std::thread::sleep;
use std::time::Duration;

use anyhow::Error;

use CurrentClipboard::{Empty, Image, Text};

pub enum CurrentClipboard {
    Text(String),
    Image(arboard::ImageData<'static>),
    Empty,
}

pub fn get_current_clipboard(clipboard: &mut Clipboard) -> CurrentClipboard {
    if let Ok(text) = clipboard.get_text() {
        Text(text)
    } else if let Ok(img) = clipboard.get_image() {
        Image(img)
    } else {
        Empty
    }
}

pub fn get_text_from_selection(
    write_conn: &mut Enigo,
    clipboard: &mut Clipboard,
) -> Result<String, Error> {
    write_conn.key(Key::Meta, Direction::Press)?;
    write_conn.key(Key::Unicode('c'), Direction::Click)?;
    write_conn.key(Key::Meta, Direction::Release)?;

    sleep(Duration::from_millis(100));

    let text = Clipboard::get_text(clipboard)?;

    Ok(text)
}

pub fn set_clipboard(
    content: CurrentClipboard,
    clipboard: &mut Clipboard,
) -> Result<(), arboard::Error> {
    let new_clip = Clipboard::set(clipboard);

    if let CurrentClipboard::Text(text) = content {
        new_clip.text(text)?;
    } else if let CurrentClipboard::Image(img) = content {
        new_clip.image(img)?;
    }
    Ok(())
}
