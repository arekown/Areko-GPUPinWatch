//! Tray icon: the plug symbol tinted green / yellow / red in the colours of the chosen theme,
//! blinking (1 Hz, like the draft) while a card is throttled or in alarm. The single menu
//! entry carries the state line (tooltips are not shown on Linux) and opens the window.
use crate::alerts::TrayState;
use gpw_core::policy::Phase;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};

/// White plug symbol on transparent background, rendered from icons/tray.svg.
const MASK_PNG: &[u8] = include_bytes!("../icons/tray.png");
const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";

/// ok / warn / bad / dim per theme (same values as the CSS tokens in ui/style.css).
fn colour(theme: &str, st: Option<TrayState>) -> [u8; 3] {
    let (ok, warn, bad, dim) = match theme {
        "light" => ([0x16, 0xa3, 0x4a], [0xd9, 0x77, 0x06], [0xdc, 0x26, 0x26], [0x6b, 0x76, 0x86]),
        "dark" => ([0x3f, 0xb9, 0x50], [0xd2, 0x99, 0x22], [0xf8, 0x51, 0x49], [0x8b, 0x94, 0x9e]),
        _ => ([0x00, 0xff, 0x9f], [0xf9, 0xf0, 0x02], [0xff, 0x2a, 0x6d], [0x5f, 0x7d, 0x86]),
    };
    match st {
        None => dim,
        Some(s) if s.cards == 0 => dim,
        Some(s) => match s.phase {
            Phase::Ok => ok,
            Phase::Throttled => warn,
            Phase::Alarm => bad,
        },
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Look {
    rgb: [u8; 3],
    faded: bool,
}

struct Inner {
    theme: String,
    state: Option<TrayState>,
    line: String,
    blink_on: bool,
    shown: Option<Look>,
}

#[derive(Clone)]
pub struct Tray {
    icon: TrayIcon<Wry>,
    item: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    mask: Arc<(Vec<u8>, u32, u32)>,
    inner: Arc<Mutex<Inner>>,
}

impl Tray {
    pub fn create(app: &AppHandle, on_open: fn(&AppHandle)) -> tauri::Result<Tray> {
        let m = Image::from_bytes(MASK_PNG)?;
        let mask = Arc::new((m.rgba().to_vec(), m.width(), m.height()));
        let item = MenuItem::with_id(app, MENU_OPEN, "Areko GPUPinWatch", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&item, &quit])?;
        let first = Look { rgb: colour("areko", None), faded: false };
        let icon = TrayIconBuilder::with_id("main")
            .icon(tint(&mask, first))
            .tooltip("Areko GPUPinWatch")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(move |app, e| {
                if e.id() == MENU_OPEN {
                    on_open(app);
                } else if e.id() == MENU_QUIT {
                    app.exit(0);
                }
            })
            .on_tray_icon_event(move |tray, e| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    on_open(tray.app_handle());
                }
            })
            .build(app)?;
        let tray = Tray {
            icon,
            item,
            quit,
            mask,
            inner: Arc::new(Mutex::new(Inner {
                theme: "areko".into(),
                state: None,
                line: String::new(),
                blink_on: true,
                shown: Some(first),
            })),
        };
        let t = tray.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            t.tick();
        });
        Ok(tray)
    }

    /// Translated label of the "Quit" menu entry.
    pub fn set_quit_text(&self, text: &str) {
        let _ = self.quit.set_text(text);
    }

    /// New service state (`None` = not connected), theme from the service config, translated line.
    pub fn update(&self, state: Option<TrayState>, theme: Option<&str>, line: String) {
        let mut g = self.inner.lock().unwrap();
        g.state = state;
        if let Some(t) = theme {
            g.theme = t.to_string();
        }
        if g.line != line {
            let _ = self.icon.set_tooltip(Some(&line));
            let _ = self.item.set_text(&line);
            g.line = line;
        }
        self.apply(&mut g);
    }

    fn tick(&self) {
        let mut g = self.inner.lock().unwrap();
        g.blink_on = !g.blink_on;
        self.apply(&mut g);
    }

    fn apply(&self, g: &mut Inner) {
        let blinking = matches!(g.state, Some(s) if s.cards > 0 && s.phase != Phase::Ok);
        let look = Look { rgb: colour(&g.theme, g.state), faded: blinking && !g.blink_on };
        if g.shown != Some(look) {
            let _ = self.icon.set_icon(Some(tint(&self.mask, look)));
            g.shown = Some(look);
        }
    }
}

/// Colours the white mask; the "off" half of a blink keeps 20 % opacity like the draft's animation.
fn tint(mask: &(Vec<u8>, u32, u32), look: Look) -> Image<'static> {
    let (px, w, h) = mask;
    let mut out = px.clone();
    for p in out.chunks_exact_mut(4) {
        p[..3].copy_from_slice(&look.rgb);
        if look.faded {
            p[3] = (p[3] as u16 * 2 / 10) as u8;
        }
    }
    Image::new_owned(out, *w, *h)
}
