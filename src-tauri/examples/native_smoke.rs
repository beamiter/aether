#[path = "../src/application.rs"]
mod application;
#[path = "native_smoke/workspace.rs"]
mod workspace;

use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{webview::PageLoadEvent, Manager, Webview, Wry};

fn check_webview(webview: &Webview<Wry>) -> Result<(), String> {
    let fixtures = include_str!("../../tests/raster-fixtures.json");
    let script = include_str!("native_smoke.js");
    webview
        .eval(format!("const aetherSmokeFixtures = {fixtures};\n{script}"))
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut resized = false;
    while Instant::now() < deadline {
        let (sender, receiver) = mpsc::sync_channel(1);
        webview
            .eval_with_callback(
                "globalThis.aetherSmokeResult ?? (globalThis.aetherSmokeResize ? 'resize-window' : null)",
                move |value| {
                    let _ = sender.send(value);
                },
            )
            .map_err(|error| error.to_string())?;
        let value = receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|error| format!("WebKit response: {error}"))?;
        match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(serde_json::Value::Null) => std::thread::sleep(Duration::from_millis(50)),
            Ok(serde_json::Value::Bool(true)) => return Ok(()),
            Ok(serde_json::Value::String(request)) if request == "resize-window" => {
                if !resized {
                    webview
                        .window()
                        .set_size(tauri::LogicalSize::new(700.0, 560.0))
                        .map_err(|error| error.to_string())?;
                    resized = true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(serde_json::Value::String(error)) => return Err(error),
            _ => return Err(format!("Unexpected WebKit response: {value:?}")),
        }
    }
    Err("Timed out waiting for native smoke assertions".into())
}

fn main() {
    // The example can start an isolated shared host for its own fixtures.
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--jterm-session-host")) {
        std::process::exit(jterm_core::agent_workspace::run_embedded_if_requested().unwrap_or(2));
    }
    let fixture = workspace::Fixture::start().expect("Isolated Agent Workspace fixture");
    let app = application::configure(tauri::Builder::default(), fixture.workspace())
        .on_page_load(|webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let webview = webview.clone();
            std::thread::spawn(move || {
                let code = match check_webview(&webview) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!("Native smoke failed: {error}");
                        1
                    }
                };
                webview.app_handle().exit(code);
            });
        })
        .build(tauri::generate_context!())
        .expect("Tauri runtime");
    let code = app.run_return(|_, _| {});
    if code == 0 {
        fixture
            .verify_provider_source()
            .expect("Provider history is preserved");
        println!("Mapped Tauri and WebKit: PTY terminfo, window fitting, Workspace search and deletion, and file previews passed.");
    }
    drop(fixture);
    std::process::exit(code);
}
