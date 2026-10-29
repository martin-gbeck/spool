use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 32;

pub fn build_tray() -> anyhow::Result<TrayIcon> {
    let icon = Icon::from_rgba(speaker_icon(ICON_SIZE), ICON_SIZE, ICON_SIZE)?;

    let menu = Menu::new();

    menu.append(&MenuItem::with_id("pause", "Pause", true, None))?;
    menu.append(&MenuItem::with_id("play", "Resume", true, None))?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id("skip", "Skip this chunk", true, None))?;
    menu.append(&MenuItem::with_id("stop", "Stop", true, None))?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id("open_config", "Open config", true, None))?;
    menu.append(&MenuItem::with_id("quit", "Quit", true, None))?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("rust tts")
        .with_icon(icon)
        // macOS recolours a template icon to match the menu bar, light or dark,
        // which is why the glyph below is drawn as black plus an alpha mask.
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .build()?;

    Ok(tray)
}

/// A speaker with two sound waves, drawn rather than shipped as a file so the
/// binary stays the only thing that needs installing. Coordinates are unit
/// fractions of the icon, sampled 4x4 per pixel for smooth edges.
fn speaker_icon(size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;

    let mut rgba = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let mut hits = 0u32;

            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;

                    if in_glyph(u, v) {
                        hits += 1;
                    }
                }
            }

            let idx = ((y * size + x) * 4) as usize;
            rgba[idx + 3] = (255 * hits / (SAMPLES * SAMPLES)) as u8;
        }
    }

    rgba
}

fn in_glyph(u: f32, v: f32) -> bool {
    const CONE_BACK: f32 = 0.30;
    const CONE_FRONT: f32 = 0.50;

    // Body: the rectangle behind the cone.
    if (0.15..=CONE_BACK).contains(&u) && (0.40..=0.60).contains(&v) {
        return true;
    }

    // Cone: half-height grows from 0.10 at the body to 0.30 at the front.
    if (CONE_BACK..=CONE_FRONT).contains(&u) {
        let spread = 0.10 + (u - CONE_BACK) / (CONE_FRONT - CONE_BACK) * 0.20;
        if (v - 0.5).abs() <= spread {
            return true;
        }
    }

    // Waves: two arcs in front of the cone, opening to the right.
    let (dx, dy) = (u - CONE_FRONT, v - 0.5);
    if dx <= 0.0 {
        return false;
    }

    let radius = (dx * dx + dy * dy).sqrt();
    if dy.atan2(dx).abs() > 0.9 {
        return false; // roughly 50 degrees either side of straight ahead
    }

    (radius - 0.17).abs() <= 0.028 || (radius - 0.29).abs() <= 0.028
}
