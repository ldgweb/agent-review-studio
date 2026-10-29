// Agent Review Studio: the desktop shell.
// The shell hosts the native core (src/core): ingestion adapters, the
// heuristic risk engine, SQLite store and git integration all run
// in-process, exposed to the React frontend through a single `api` command
// (no HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The native core lives in the library crate (src/lib.rs), shared with the
// `ars` CLI (src/bin/ars.rs).
use agent_review_studio::app_core;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
async fn api(path: String, method: Option<String>, body: Option<String>) -> Result<String, String> {
    let method = method.unwrap_or_else(|| "GET".to_string());
    let response = app_core::api::handle(&path, &method, body.as_deref()).await;
    serde_json::to_string(&response).map_err(|e| e.to_string())
}

/// The project `ars <dir>` asked to open: `--project <path>` (argv without
/// the program name).
fn requested_project(args: &[String]) -> Option<&str> {
    let at = args.iter().position(|a| a == "--project")?;
    args.get(at + 1)
        .map(String::as_str)
        .filter(|p| !p.is_empty())
}

/// The frontend route that opens a project's review. The path is
/// percent-encoded, so the result is plain ASCII and safe to embed as a
/// JSON string in a script.
fn project_route(project: &str) -> String {
    let route = format!(
        "#/project?path={}",
        utf8_percent_encode(project, NON_ALPHANUMERIC)
    );
    serde_json::to_string(&route).unwrap_or_else(|_| "\"#/\"".into())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch_project = requested_project(&args).map(String::from);

    tauri::Builder::default()
        // First, so a second launch exits before it builds anything.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            let args: Vec<String> = argv.into_iter().skip(1).collect();
            if let Some(project) = requested_project(&args) {
                let _ = window.eval(format!("location.hash = {};", project_route(project)));
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![api])
        .setup(move |app| {
            // The core pushes changes (sessions, the reviewed project) as
            // events: the window never polls.
            let handle = app.handle().clone();
            app_core::events::set_emitter(move |name, payload| {
                let _ = handle.emit(name, payload);
            });
            // Background scanning (watchers + debounce + slow tick) belongs to
            // the desktop only; it also warms the first scan while the
            // webview is still booting.
            app_core::scanner::start_background();
            let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Agent Review Studio")
                .inner_size(1440.0, 900.0)
                .min_inner_size(1000.0, 640.0)
                // No Ctrl+wheel / Ctrl+± page zoom: this is an app, not a page.
                .zoom_hotkeys_enabled(false);
            // The app draws its own title bar. On macOS the system's window
            // buttons stay, over the left end of it; elsewhere the bar has
            // its own.
            #[cfg(target_os = "macos")]
            {
                builder = builder
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true)
                    // Centered in the app's 40px bar
                    .traffic_light_position(tauri::LogicalPosition::new(14.0, 13.0));
            }
            #[cfg(not(target_os = "macos"))]
            {
                builder = builder.decorations(false);
            }
            if let Some(project) = &launch_project {
                // Runs before the app's scripts, so the router starts on the
                // project route. Only an empty route is replaced.
                builder = builder.initialization_script(format!(
                    "if (!location.hash || location.hash === '#/') location.hash = {};",
                    project_route(project)
                ));
            }
            builder.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building Agent Review Studio");
}

#[cfg(test)]
mod tests {
    use super::{project_route, requested_project};

    #[test]
    fn project_flag_and_route_are_parsed_and_encoded() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            requested_project(&args(&["--project", "C:/a b"])),
            Some("C:/a b")
        );
        assert_eq!(requested_project(&args(&["--project"])), None);
        assert_eq!(requested_project(&args(&[])), None);
        // Quotes, backslashes and script-breaking characters never survive raw.
        let route = project_route("C:\\x\";alert(1)//</script>");
        assert!(
            route.starts_with("\"#/project?path=C%3A%5Cx%22%3Balert"),
            "{route}"
        );
        assert!(
            !route[1..route.len() - 1].contains(['"', '\\', '<', ';']),
            "{route}"
        );
    }
}
