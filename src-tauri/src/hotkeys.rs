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

pub fn bindings() -> Vec<(Shortcut, Action, &'static str)> {
    let cs = Some(Modifiers::CONTROL | Modifiers::SHIFT);
    vec![
        (Shortcut::new(cs, Code::KeyH), Action::ToggleWindow, "Ctrl+Shift+H"),
        (Shortcut::new(cs, Code::KeyC), Action::Clear, "Ctrl+Shift+C"),
        (Shortcut::new(cs, Code::ArrowUp), Action::ScrollUp, "Ctrl+Shift+Up"),
        (Shortcut::new(cs, Code::ArrowDown), Action::ScrollDown, "Ctrl+Shift+Down"),
        (Shortcut::new(cs, Code::KeyP), Action::TogglePause, "Ctrl+Shift+P"),
        (Shortcut::new(cs, Code::KeyM), Action::ToggleClickThrough, "Ctrl+Shift+M"),
        (Shortcut::new(cs, Code::KeyS), Action::OpenSettings, "Ctrl+Shift+S"),
    ]
}

/// Registers every hotkey; returns the labels of those that could not be registered
/// (e.g. already taken by another program).
pub fn init(app: &AppHandle) -> tauri::Result<Vec<&'static str>> {
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
