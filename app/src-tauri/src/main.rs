// Agent Review Studio — desktop shell.
// The shell hosts the native core (src/core): ingestion adapters, the
// heuristic risk engine, SQLite store and git integration all run
// in-process, exposed to the React frontend through a single `api` command
// (no HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The native core lives in the library crate (src/lib.rs), shared with the
// `ars` CLI (src/bin/ars.rs).
use agent_review_studio::app_core;

use tauri::{WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
async fn api(path: String, method: Option<String>, body: Option<String>) -> Result<String, String> {
    let method = method.unwrap_or_else(|| "GET".to_string());
    let response = app_core::api::handle(&path, &method, body.as_deref()).await;
    serde_json::to_string(&response).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![api])
        .setup(move |app| {
            // Background scanning (watchers + debounce + slow tick) belongs to
            // the desktop only; it also warms the first scan while the
            // webview is still booting.
            app_core::scanner::start_background();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Agent Review Studio")
                .inner_size(1440.0, 900.0)
                .min_inner_size(1000.0, 640.0)
                // The app draws its own title bar — no browser-window chrome.
                .decorations(false)
                // No Ctrl+wheel / Ctrl+± page zoom: this is an app, not a page.
                .zoom_hotkeys_enabled(false)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building Agent Review Studio");
}
