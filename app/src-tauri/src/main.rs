//! Areko GPUPinWatch window (Tauri 2). Frontend: ../ui (HTML/CSS/JS from the accepted draft).
//! A background thread subscribes to the service and forwards every snapshot as the event
//! "gpw-snapshot"; connection changes go out as "gpw-service", new throttle/alarm phases as "gpw-alert".
//!
//! Required Notice: Copyright Areko (https://github.com/arekown/Areko-GPUPinWatch)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod ipc_client;
mod tray;

use alerts::{Texts, Watch};
use gpw_core::config::Config;
use gpw_core::ipc::{Request, Snapshot};
use gpw_core::policy::Phase;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;

/// Started by the system autostart: stay in the tray.
const HIDDEN_ARG: &str = "--hidden";
const REPO_URL: &str = "https://github.com/arekown/Areko-GPUPinWatch";

#[derive(Default)]
struct AppState {
    connected: Mutex<Option<bool>>,
    last: Mutex<Option<Snapshot>>,
    logged_cards: Mutex<Option<usize>>,
    watch: Mutex<Watch>,
    texts: Mutex<Texts>,
    tray: Mutex<Option<tray::Tray>>,
}

#[derive(Serialize)]
struct StateView {
    connected: bool,
    snapshot: Option<Snapshot>,
}

#[derive(Serialize, Clone)]
struct AlertView {
    phase: Phase,
}

fn log(msg: &str) {
    eprintln!("areko-gpupinwatch: {msg}");
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn refresh_tray(app: &AppHandle, snap: Option<&Snapshot>) {
    let st = app.state::<AppState>();
    let ts = snap.map(alerts::tray_state);
    let line = st.texts.lock().unwrap().tray_line(ts);
    let tray = st.tray.lock().unwrap().clone();
    if let Some(t) = tray {
        t.update(ts, snap.map(|s| s.config.theme.as_str()), line);
    }
}

/// Called by the subscription thread for every snapshot (`None` = service not reachable).
fn on_service(app: &AppHandle, snap: Option<Snapshot>, why: &str) {
    let st = app.state::<AppState>();
    let now = snap.is_some();
    let changed = st.connected.lock().unwrap().replace(now) != Some(now);
    if changed {
        if now {
            log(&format!("connected to {}", ipc_client::endpoint()));
        } else {
            log(&format!("service not reachable ({why}), retrying every 2 s"));
            *st.logged_cards.lock().unwrap() = None;
        }
        let _ = app.emit("gpw-service", now);
    }
    let Some(snap) = snap else {
        *st.last.lock().unwrap() = None;
        refresh_tray(app, None);
        return;
    };
    {
        let mut logged = st.logged_cards.lock().unwrap();
        if *logged != Some(snap.cards.len()) {
            let names: Vec<String> = snap.cards.iter().map(alerts::card_title).collect();
            log(&format!("snapshot with {} cards: {}", snap.cards.len(), names.join(", ")));
            *logged = Some(snap.cards.len());
        }
    }
    let new_alerts = st.watch.lock().unwrap().step(&snap);
    refresh_tray(app, Some(&snap));
    for a in &new_alerts {
        if snap.config.desktop_alarm {
            let (title, body) = st.texts.lock().unwrap().notification(a);
            if let Err(e) = app.notification().builder().title(title).body(body).show() {
                log(&format!("desktop notification failed: {e}"));
            }
        }
        let _ = app.emit("gpw-alert", AlertView { phase: a.phase });
        if a.phase == Phase::Alarm {
            show_main(app);
        }
    }
    let _ = app.emit("gpw-snapshot", &snap);
    *st.last.lock().unwrap() = Some(snap);
}

#[tauri::command]
fn get_state(state: tauri::State<'_, AppState>) -> StateView {
    StateView {
        connected: state.connected.lock().unwrap().unwrap_or(false),
        snapshot: state.last.lock().unwrap().clone(),
    }
}

/// Translated templates for notifications and the tray line (sent on every language change).
#[tauri::command]
fn set_texts(app: AppHandle, texts: HashMap<String, String>) {
    let st = app.state::<AppState>();
    *st.texts.lock().unwrap() = Texts(texts);
    let last = st.last.lock().unwrap().clone();
    refresh_tray(&app, last.as_ref());
}

async fn send(req: Request) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || ipc_client::request(&req)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn release(pci: String) -> Result<(), String> {
    send(Request::Release { pci }).await
}

#[tauri::command]
async fn set_config(config: Config) -> Result<(), String> {
    send(Request::SetConfig { config }).await
}

#[tauri::command]
fn autostart_get(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_set(app: AppHandle, on: bool) -> Result<(), String> {
    let a = app.autolaunch();
    if on { a.enable() } else { a.disable() }.map_err(|e| e.to_string())
}

/// The source code link on the info page opens in the system browser.
#[tauri::command]
fn open_repo(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(REPO_URL, None::<&str>).map_err(|e| e.to_string())
}

fn main() {
    // WebKitGTK's DMA-BUF renderer aborts with "Error 71 (protocol error)" on Wayland with the
    // NVIDIA driver; the shared-memory path works everywhere. Must be set before GTK starts.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec![HIDDEN_ARG])))
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![get_state, set_texts, release, set_config, autostart_get, autostart_set, open_repo])
        .on_window_event(|w, e| {
            // closing the window keeps watching in the tray
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let t = tray::Tray::create(&handle, show_main)?;
            *app.state::<AppState>().tray.lock().unwrap() = Some(t);
            if std::env::args().any(|a| a == HIDDEN_ARG) {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            std::thread::spawn(move || ipc_client::subscribe(|snap, why| on_service(&handle, snap, why)));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Areko GPUPinWatch");
}
