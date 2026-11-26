//! Real command macros, deserialization and managed state with a mock window runtime.
//! The shell is a real Linux PTY; these are not WebKit/window/IME tests.
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{test::MockRuntime, Manager};
use tauri_terminal_frame_bridge::live::LiveBridge;

use super::application::{self, Workspace};

fn request(command: &str, body: Value) -> tauri::webview::InvokeRequest {
    tauri::webview::InvokeRequest {
        cmd: command.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: tauri::ipc::InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.into(),
    }
}

fn invoke(window: &tauri::WebviewWindow<MockRuntime>, command: &str, body: Value) -> Value {
    tauri::test::get_ipc_response(window, request(command, body))
        .unwrap()
        .deserialize::<Value>()
        .unwrap()
}

#[test]
fn command_dispatch_returns_while_backend_lock_is_busy() {
    let app = application::configure(tauri::test::mock_builder(), Workspace::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let handle = app.handle().clone();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let state = handle.state::<std::sync::Arc<std::sync::Mutex<LiveBridge>>>();
        let _guard = state.lock().unwrap();
        locked_tx.send(()).unwrap();
        // A broken synchronous handler must fail rather than deadlock the test.
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
    });
    locked_rx.recv().unwrap();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let start = Instant::now();
    window.as_ref().clone().on_message(
        request("shell_status", json!({"epoch": 1})),
        Box::new(move |_, _, response, _, _| response_tx.send(response).unwrap()),
    );
    let elapsed = start.elapsed();
    let _ = release_tx.send(());
    holder.join().unwrap();
    assert!(
        elapsed < Duration::from_secs(1),
        "dispatch blocked: {elapsed:?}"
    );
    assert!(matches!(
        response_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        tauri::ipc::InvokeResponse::Err(_)
    ));
}

#[test]
fn tauri_ipc_real_pty_lifecycle_and_wire_contract() {
    let app = application::configure(tauri::test::mock_builder(), Workspace::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let epoch = invoke(&window, "open_shell", json!({})).as_u64().unwrap();
    assert!(
        tauri::test::get_ipc_response(&window, request("poll_shell", json!({"epoch": "bad"})))
            .is_err()
    );
    assert!(tauri::test::get_ipc_response(
        &window,
        request("write_shell", json!({"epoch": epoch + 1, "bytes": [120]}))
    )
    .is_err());
    invoke(
        &window,
        "resize_shell",
        json!({"epoch": epoch, "rows": 9, "cols": 64}),
    );
    invoke(
        &window,
        "text_shell",
        json!({"epoch": epoch, "text": "printf 'IPC_%s\\n' VERIFIED; exit"}),
    );
    invoke(
        &window,
        "key_shell",
        json!({"epoch": epoch, "name": "Enter", "shift": false, "alt": false, "ctrl": false}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "real PTY did not drain");
        let frame = invoke(&window, "poll_shell", json!({"epoch": epoch}));
        if !frame.is_null() {
            invoke(
                &window,
                "ack_shell",
                json!({"epoch": epoch, "revision": frame["revision"]}),
            );
        }
        if invoke(&window, "shell_status", json!({"epoch": epoch}))["drained"] == true {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let frame = invoke(&window, "recover_shell", json!({"epoch": epoch}));
    assert_eq!(frame["epoch"], epoch);
    assert_eq!(frame["rows"].as_array().unwrap().len(), 9);
    assert!(frame["rows"].as_array().unwrap().iter().any(|row| {
        row.as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect::<String>()
            .contains("IPC_VERIFIED")
    }));
    invoke(
        &window,
        "ack_shell",
        json!({"epoch": epoch, "revision": frame["revision"]}),
    );
    assert_eq!(
        invoke(
            &window,
            "history_shell",
            json!({"epoch": epoch, "offset": 0})
        )["epoch"],
        epoch
    );
    assert!(
        invoke(&window, "close_shell", json!({"epoch": epoch}))["cleanup_pending"].is_boolean()
    );
    assert!(
        tauri::test::get_ipc_response(&window, request("poll_shell", json!({"epoch": epoch})))
            .is_err()
    );
    for invalid in [json!({"rows": 14}), json!({"rows": 0, "cols": 72})] {
        assert!(tauri::test::get_ipc_response(&window, request("open_shell", invalid)).is_err());
    }
    let next = invoke(&window, "open_shell", json!({"rows": 14, "cols": 72}))
        .as_u64()
        .unwrap();
    assert!(next > epoch);
    let first = invoke(&window, "poll_shell", json!({"epoch": next}));
    assert_eq!(first["rows"].as_array().unwrap().len(), 14);
    assert_eq!(first["cols"], 72);
    invoke(&window, "close_shell", json!({"epoch": next}));
}

#[test]
fn tauri_mouse_command_delivers_the_displayed_mode_to_the_pty() {
    let app = application::configure(tauri::test::mock_builder(), Workspace::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let epoch = invoke(&window, "open_shell", json!({}));
    let command = concat!(
        "stty -echo -icanon min 1 time 0; printf '\\033[?1000;1006h'; ",
        "report=$(dd bs=1 count=9 2>/dev/null | od -An -tx1 -v | tr -d ' \\n'); ",
        "printf '\\nMOUSE_RX=%s\\n' \"$report\"; exit",
    );
    invoke(
        &window,
        "text_shell",
        json!({"epoch": epoch, "text": command}),
    );
    invoke(
        &window,
        "key_shell",
        json!({"epoch": epoch, "name": "Enter", "shift": false, "alt": false, "ctrl": false}),
    );

    let row_text = |row: &Value| {
        row.as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect::<String>()
    };
    let wait_for_frame = |what, matches: &dyn Fn(&Value) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = Value::Null;
        loop {
            let frame = invoke(&window, "poll_shell", json!({"epoch": epoch}));
            if !frame.is_null() {
                invoke(
                    &window,
                    "ack_shell",
                    json!({"epoch": epoch, "revision": frame["revision"]}),
                );
                if matches(&frame) {
                    return frame;
                }
                last = frame;
            }
            if Instant::now() >= deadline {
                let rows = last["rows"].as_array().map(|rows| {
                    rows.iter()
                        .map(row_text)
                        .filter(|row| !row.trim().is_empty())
                        .collect::<Vec<_>>()
                });
                panic!(
                    "{what} missing; last mouse mode {}, rows {rows:?}",
                    last["mouse"]
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let active = wait_for_frame("Mouse tracking", &|frame| {
        frame["mouse"]["tracking"] == "button"
    });
    let mut report = json!({
        "kind": "press", "layout": active["layout"], "generation": active["mouse"]["generation"],
        "row": 2, "col": 4, "button": "left", "alt": false, "ctrl": false,
    });
    report["generation"] = json!(active["mouse"]["generation"].as_u64().unwrap() + 1);
    assert_eq!(
        invoke(
            &window,
            "mouse_shell",
            json!({"epoch": epoch, "report": report})
        ),
        false
    );
    report["generation"] = active["mouse"]["generation"].clone();
    assert_eq!(
        invoke(
            &window,
            "mouse_shell",
            json!({"epoch": epoch, "report": report})
        ),
        true
    );
    wait_for_frame("Exact mouse receipt", &|frame| {
        frame["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row_text(row).trim_end() == "MOUSE_RX=1b5b3c303b353b334d")
    });
    invoke(&window, "close_shell", json!({"epoch": epoch}));
}
