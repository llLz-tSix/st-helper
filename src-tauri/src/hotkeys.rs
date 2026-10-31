//! All global hotkeys live here.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[derive(Clone, Copy, Debug)]
pub enum Action {
    ToggleWindow,
    Clear,
    ScrollUp,
    ScrollDown,
    TogglePause,
    ToggleClickThrough,
    OpenSettings,
}

pub fn settings_hint() -> String {
    format!("Укажите ключ Groq в настройках ({})", settings_shortcut())
}

#[cfg(target_os = "macos")]
pub fn settings_shortcut() -> &'static str {
    "Cmd+Shift+S"
}

#[cfg(not(target_os = "macos"))]
pub fn settings_shortcut() -> &'static str {
    "Ctrl+Shift+S"
}

pub fn bindings() -> Vec<(Shortcut, Action, String)> {
    #[cfg(target_os = "macos")]
    let (modifiers, modifier_label) = (Modifiers::SUPER | Modifiers::SHIFT, "Cmd+Shift");
    #[cfg(not(target_os = "macos"))]
    let (modifiers, modifier_label) = (Modifiers::CONTROL | Modifiers::SHIFT, "Ctrl+Shift");

    let cs = Some(modifiers);
    vec![
        (
            Shortcut::new(cs, Code::KeyH),
            Action::ToggleWindow,
            format!("{modifier_label}+H"),
        ),
        (
            Shortcut::new(cs, Code::KeyC),
            Action::Clear,
            format!("{modifier_label}+C"),
        ),
        (
            Shortcut::new(cs, Code::ArrowUp),
            Action::ScrollUp,
            format!("{modifier_label}+Up"),
        ),
        (
            Shortcut::new(cs, Code::ArrowDown),
            Action::ScrollDown,
            format!("{modifier_label}+Down"),
        ),
        (
            Shortcut::new(cs, Code::KeyP),
            Action::TogglePause,
            format!("{modifier_label}+P"),
        ),
        (
            Shortcut::new(cs, Code::KeyM),
            Action::ToggleClickThrough,
            format!("{modifier_label}+M"),
        ),
        (
            Shortcut::new(cs, Code::KeyS),
            Action::OpenSettings,
            format!("{modifier_label}+S"),
        ),
    ]
}

/// Registers every hotkey; returns the labels of those that could not be registered
/// (e.g. already taken by another program).
pub fn init(app: &AppHandle) -> tauri::Result<Vec<String>> {
    let binds = bindings();
    let handler_binds: Vec<(Shortcut, Action)> = binds.iter().map(|(s, a, _)| (*s, *a)).collect();
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, shortcut, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                if let Some((_, action)) = handler_binds.iter().find(|(s, _)| s == shortcut) {
                    crate::handle_action(app, *action);
                }
            })
            .build(),
    )?;
    let mut failed = Vec::new();
    for (s, _, label) in binds {
        if let Err(e) = app.global_shortcut().register(s) {
            eprintln!("hotkey {label} not registered: {e}");
            failed.push(label);
        }
    }
    Ok(failed)
}
