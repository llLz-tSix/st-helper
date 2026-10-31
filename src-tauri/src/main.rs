#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod audio;
mod groq;
mod hotkeys;
mod pipeline;
mod settings;

use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent};

use hotkeys::Action;
use settings::Settings;

pub struct Shared {
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub paused: AtomicBool,
    pub click_through: AtomicBool,
    /// Last few (role, content) dialogue turns sent to the LLM as context.
    pub history: Mutex<VecDeque<(String, String)>>,
    /// Last id handed to a generation task.
    pub gen_id: AtomicU64,
    /// Id of the generation that owns the answer area; older ones stop.
    pub active_id: AtomicU64,
    pub inflight: AtomicUsize,
    pub generation_cancel: Mutex<Option<(u64, tokio::sync::watch::Sender<bool>)>>,
    pub status: Mutex<(String, String)>,
    pub http: reqwest::Client,
}

pub fn set_status(app: &AppHandle, shared: &Shared, state: &str, text: &str) {
    *shared.status.lock().unwrap() = (state.to_string(), text.to_string());
    let _ = app.emit("status", json!({ "state": state, "text": text }));
}

/// Status when nothing special is happening.
pub fn base_status(app: &AppHandle, shared: &Shared) {
    let (state, text) = if shared.paused.load(Ordering::Relaxed) {
        ("paused", "Пауза".to_string())
    } else if shared.settings.lock().unwrap().groq_key.is_empty() {
        ("error", hotkeys::settings_hint())
    } else if shared.inflight.load(Ordering::SeqCst) > 0 {
        ("generating", "Генерирую ответ…".to_string())
    } else {
        ("listening", "Слушаю".to_string())
    };
    set_status(app, shared, state, &text);
}

/// Ask the platform to exclude the window from screen capture when supported.
fn protect(w: &WebviewWindow) {
    // Debug builds only: ST_HELPER_NO_PROTECT=1 lets you screenshot the UI.
    if cfg!(debug_assertions) && std::env::var_os("ST_HELPER_NO_PROTECT").is_some() {
        let _ = w.set_content_protected(false);
        return;
    }
    let _ = w.set_content_protected(true);
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR,
        };
        unsafe {
            let h = hwnd.0 as _;
            if SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) == 0 {
                SetWindowDisplayAffinity(h, WDA_MONITOR);
            }
        }
    }
}

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        protect(&w);
        let _ = w.emit("settings-open", ());
    }
}

pub fn handle_action(app: &AppHandle, action: Action) {
    let shared = app.state::<Arc<Shared>>();
    let Some(main) = app.get_webview_window("main") else { return };
    match action {
        Action::ToggleWindow => {
            if main.is_visible().unwrap_or(true) {
                let _ = main.hide();
            } else {
                let _ = main.show();
                protect(&main);
            }
        }
        Action::Clear => {
            let n = shared.gen_id.fetch_add(1, Ordering::SeqCst) + 1;
            shared.active_id.fetch_max(n, Ordering::SeqCst);
            if let Some((_, cancel)) = shared.generation_cancel.lock().unwrap().take() {
                let _ = cancel.send(true);
            }
            let _ = app.emit("clear", ());
        }
        Action::ScrollUp => {
            let _ = app.emit("scroll", -1);
        }
        Action::ScrollDown => {
            let _ = app.emit("scroll", 1);
        }
        Action::TogglePause => {
            shared.paused.fetch_xor(true, Ordering::SeqCst);
            base_status(app, &shared);
        }
        Action::ToggleClickThrough => {
            let on = !shared.click_through.fetch_xor(true, Ordering::SeqCst);
            let _ = main.set_ignore_cursor_events(on);
            protect(&main);
            let _ = app.emit("click-through", on);
        }
        Action::OpenSettings => show_settings(app),
    }
}

#[tauri::command]
fn get_settings(shared: State<Arc<Shared>>) -> serde_json::Value {
    shared.settings.lock().unwrap().public()
}

#[tauri::command]
fn get_state(shared: State<Arc<Shared>>) -> serde_json::Value {
    let s = shared.settings.lock().unwrap();
    let (state, text) = shared.status.lock().unwrap().clone();
    json!({
        "status": { "state": state, "text": text },
        "opacity": s.opacity,
        "font_size": s.font_size,
        "click_through": shared.click_through.load(Ordering::Relaxed),
        "hotkeys": hotkeys::bindings()
            .into_iter()
            .map(|binding| binding.2)
            .collect::<Vec<_>>(),
    })
}

/// `new_key`: empty keeps the stored key; `clear_key` removes it.
#[tauri::command]
fn save_settings(
    app: AppHandle,
    shared: State<Arc<Shared>>,
    mut settings: Settings,
    new_key: Option<String>,
    clear_key: Option<bool>,
) -> Result<(), String> {
    let mut cur = shared.settings.lock().unwrap();
    settings.groq_key = match new_key.map(|k| k.trim().to_string()) {
        Some(k) if !k.is_empty() => k,
        _ if clear_key == Some(true) => String::new(),
        _ => cur.groq_key.clone(),
    };
    settings.sanitize();
    settings.save(&shared.settings_path)?;
    *cur = settings;
    let _ = app.emit("ui-settings", json!({ "opacity": cur.opacity, "font_size": cur.font_size }));
    Ok(())
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
}

#[tauri::command]
fn toggle_pause(app: AppHandle) {
    handle_action(&app, Action::TogglePause);
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_state,
            save_settings,
            open_settings,
            toggle_pause
        ])
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("settings.json");
            let shared = Arc::new(Shared {
                settings: Mutex::new(Settings::load(&path)),
                settings_path: path,
                paused: AtomicBool::new(false),
                click_through: AtomicBool::new(false),
                history: Mutex::new(VecDeque::new()),
                gen_id: AtomicU64::new(0),
                active_id: AtomicU64::new(0),
                inflight: AtomicUsize::new(0),
                generation_cancel: Mutex::new(None),
                status: Mutex::new(("listening".into(), "Слушаю".into())),
                http: groq::client(),
            });
            app.manage(shared.clone());
            let handle = app.handle().clone();

            let main = app.get_webview_window("main").expect("main window");
            protect(&main);
            if let Ok(Some(m)) = main.primary_monitor() {
                let sz = m.size();
                let scale = m.scale_factor();
                let x = (sz.width as f64 / scale - 540.0).max(0.0);
                let _ = main.set_position(tauri::LogicalPosition::new(x, 40.0));
            }
            let settings_w = app.get_webview_window("settings").expect("settings window");
            protect(&settings_w);
            let sw = settings_w.clone();
            settings_w.on_window_event(move |e| {
                if let WindowEvent::CloseRequested { api, .. } = e {
                    api.prevent_close();
                    let _ = sw.hide();
                }
            });

            let failed = hotkeys::init(&handle)?;
            if !failed.is_empty() {
                set_status(&handle, &shared, "error", &format!("Не удалось занять клавиши: {}", failed.join(", ")));
            }
            if shared.settings.lock().unwrap().groq_key.is_empty() {
                set_status(&handle, &shared, "error", &hotkeys::settings_hint());
            }

            let (tx, rx) = tokio::sync::mpsc::channel(audio::UTTERANCE_QUEUE_CAPACITY);
            audio::spawn(handle.clone(), shared.clone(), tx);
            tauri::async_runtime::spawn(pipeline::run(handle, shared, rx));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
