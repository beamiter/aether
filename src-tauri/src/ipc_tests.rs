//! Real command macros, deserialization and managed state with a mock window runtime.
//! The shell is a real Linux PTY; these are not WebKit/window/IME tests.
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{test::MockRuntime, Manager};

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
fn ssh_terminal_ipc_rejects_foreign_windows_and_unadmitted_commands() {
    let app = super::application(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let other = tauri::WebviewWindowBuilder::new(&app, "other", Default::default())
        .build()
        .unwrap();
    let attach = json!({"action":{"operation":"attach"}});
    assert!(invoke(&window, "ssh_terminal", attach.clone()).is_null());
    assert!(
        tauri::test::get_ipc_response(&other, request("ssh_terminal", attach.clone())).is_err()
    );
    let mut nonlocal = request("ssh_terminal", attach);
    nonlocal.url = "https://example.invalid/".parse().unwrap();
    assert!(tauri::test::get_ipc_response(&window, nonlocal).is_err());
    for action in [
        json!({"operation":"open","destination":"-oProxyCommand=forbidden"}),
        json!({"operation":"request","command":{"kind":"open","rows":24,"cols":80}}),
        json!({"operation":"request","command":{"kind":"write","owner":{"session":"aabb","epoch":1,"generation":1},"bytes":[65]}}),
    ] {
        assert!(tauri::test::get_ipc_response(
            &window,
            request("ssh_terminal", json!({"action":action}))
        )
        .is_err());
    }
}

#[test]
fn remote_desktop_ipc_validates_window_generation_and_control_scope() {
    let app = super::application(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let other = tauri::WebviewWindowBuilder::new(&app, "other", Default::default())
        .build()
        .unwrap();
    let inspect = json!({"action": {"type":"inspect"}});
    let initial = invoke(&window, "remote_desktop", inspect.clone());
    assert_eq!(initial["generation"], 0);
    assert_eq!(initial["active"], false);
    assert_eq!(initial["embedded"], false);
    assert_eq!(initial["sources"], json!([]));
    assert_eq!(initial["requestAvailable"], false);
    assert_eq!(initial["clipboardRequested"], false);
    assert!(initial["clipboard"].is_null());
    assert!(initial.get("pixels").is_none());
    assert!(
        tauri::test::get_ipc_response(&other, request("remote_desktop", inspect.clone())).is_err()
    );
    let mut nonlocal = request("remote_desktop", inspect);
    nonlocal.url = "https://example.invalid/".parse().unwrap();
    assert!(tauri::test::get_ipc_response(&window, nonlocal).is_err());

    let rejects = |action| {
        tauri::test::get_ipc_response(
            &window,
            request("remote_desktop", json!({"action": action})),
        )
        .unwrap_err()
    };
    assert_eq!(
        rejects(json!({"type":"disconnect", "generation": 1})),
        "The remote session changed."
    );
    assert_eq!(
        rejects(json!({"type":"release_input", "generation": 0})),
        "The remote viewer is closing or disconnected."
    );
    let selection =
        json!({"session_id":"0123456789abcdef0123456789abcdef", "source_epoch":"2", "source_id":2});
    assert_eq!(
        rejects(json!({"type":"request_input", "generation":1, "selection":selection})),
        "The remote session changed."
    );
    assert_eq!(
        rejects(json!({"type":"request_input", "generation":0, "selection":selection})),
        "The remote viewer is closing or disconnected."
    );
    assert_eq!(
        rejects(json!({"type":"connect", "generation": 0, "options": {
            "address":"127.0.0.1", "keyFile":"", "viewer":"unsupported", "scaling":"auto", "control":true,
        }})),
        "Unknown viewer backend."
    );
    assert!(
        rejects(json!({"type":"connect", "generation": 0, "options": {
            "address":"127.0.0.1", "keyFile":"", "viewer":"x11", "scaling":"auto", "control":false,
            "x11ParentWindow":256,
        }}))
        .as_str()
        .is_some_and(|message| message.contains("x11ParentWindow"))
    );
    for (viewer, control) in [("x11", true), ("wayland", false)] {
        assert_eq!(
            rejects(json!({"type":"connect", "generation": 0, "options": {
                "address":"127.0.0.1", "keyFile":"", "viewer":viewer, "scaling":"auto",
                "control":control, "waylandGestures":true,
            }})),
            "Gestures require the Wayland viewer and keyboard and pointer control."
        );
    }
    assert_eq!(
        rejects(json!({"type":"connect", "generation": 0, "options": {
            "address":"127.0.0.1", "keyFile":"", "viewer":"wayland", "scaling":"auto", "control":false,
            "clipboard":{"mode":"text", "direction":"from-host", "fileLimitBytes":"0"},
        }})),
        "Clipboard sharing requires the X11 viewer."
    );
    for (viewer, control, clipboard) in [
        ("wayland", false, false),
        ("x11", true, false),
        ("x11", false, true),
    ] {
        let mut options = json!({
            "address":"127.0.0.1", "keyFile":"", "viewer":viewer, "scaling":"auto",
            "control":control, "reconnectAutomatically":true,
        });
        if clipboard {
            options["clipboard"] =
                json!({"mode":"text", "direction":"from-host", "fileLimitBytes":"0"});
        }
        assert_eq!(
            rejects(json!({"type":"connect", "generation":0, "options":options})),
            "Automatic reconnect requires view-only X11 with clipboard sharing off."
        );
    }
    let invalid_cap = rejects(json!({"type":"connect", "generation": 0, "options": {
        "address":"127.0.0.1", "keyFile":"", "viewer":"x11", "scaling":"auto", "control":false,
        "clipboard":{"mode":"all", "direction":"both", "fileLimitBytes":"18446744073709551616"},
    }}));
    assert!(invalid_cap
        .as_str()
        .is_some_and(|message| message.contains("Clipboard byte count exceeds")));
}

#[test]
fn command_dispatch_returns_while_backend_lock_is_busy() {
    let app = super::application(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let handle = app.handle().clone();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let state = handle.state::<std::sync::Arc<std::sync::Mutex<super::LocalSessions>>>();
        let _guard = state.lock().unwrap();
        locked_tx.send(()).unwrap();
        // A broken synchronous handler must fail rather than deadlock the test.
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
    });
    locked_rx.recv().unwrap();
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let start = Instant::now();
    window.as_ref().clone().on_message(
        request("shell_status", json!({"owner":{"epoch":1,"session":"00000000000000000000000000000001","generation":1}})),
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
    let app = super::application(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    assert!(invoke(&window, "attach_shell", json!({})).is_null());
    let owner = invoke(&window, "open_shell", json!({}));
    let epoch = owner["epoch"].as_u64().unwrap();
    assert_eq!(owner["session"].as_str().unwrap().len(), 32);
    assert!(
        tauri::test::get_ipc_response(&window, request("poll_shell", json!({"owner": "bad"})))
            .is_err()
    );
    let mut future = owner.clone();
    future["epoch"] = json!(epoch + 1);
    assert!(tauri::test::get_ipc_response(
        &window,
        request("write_shell", json!({"owner": future, "bytes": [120]}))
    )
    .is_err());

    invoke(
        &window,
        "resize_shell",
        json!({"owner": owner, "rows": 9, "cols": 64}),
    );
    // An admitted logical event keeps ordinary raw backpressure. The same
    // local IPC may retry its exact LF, but cannot substitute another byte.
    invoke(
        &window,
        "text_shell",
        json!({"owner": owner, "text": "true"}),
    );
    let raw_suffix = json!({"owner": owner, "bytes": [10]});
    assert_eq!(invoke(&window, "write_shell", raw_suffix.clone()), 0);
    assert!(tauri::test::get_ipc_response(
        &window,
        request("write_shell", json!({"owner": owner, "bytes": [88]})),
    )
    .is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let update = invoke(&window, "poll_shell", json!({"owner": owner}));
        if !update.is_null() {
            invoke(
                &window,
                "ack_shell",
                json!({"owner": owner, "revision": update["frame"]["revision"]}),
            );
        }
        let accepted = invoke(&window, "write_shell", raw_suffix.clone());
        if accepted == 1 {
            break;
        }
        assert_eq!(accepted, 0);
        assert!(Instant::now() < deadline, "owned raw suffix stayed blocked");
        std::thread::sleep(Duration::from_millis(2));
    }
    invoke(
        &window,
        "text_shell",
        json!({"owner": owner, "text": "printf 'IPC_%s\\n' VERIFIED; exit"}),
    );
    invoke(
        &window,
        "key_shell",
        json!({"owner": owner, "name": "Enter", "shift": false, "alt": false, "ctrl": false}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "real PTY did not drain");
        let update = invoke(&window, "poll_shell", json!({"owner": owner}));
        if !update.is_null() {
            assert_eq!(update["frame"]["session"], owner["session"]);
            invoke(
                &window,
                "ack_shell",
                json!({"owner": owner, "revision": update["frame"]["revision"]}),
            );
        }
        if invoke(&window, "shell_status", json!({"owner": owner}))["drained"] == true {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let recovered = invoke(&window, "recover_shell", json!({"owner": owner}));
    let current = &recovered["owner"];
    let frame = &recovered["frame"];
    assert_eq!(frame["epoch"], epoch);
    assert_eq!(current["session"], owner["session"]);
    assert_eq!(current["generation"], 2);
    assert_eq!(frame["rows"].as_array().unwrap().len(), 9);
    assert!(frame["rows"].as_array().unwrap().iter().any(|row| {
        row.as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect::<String>()
            .contains("IPC_VERIFIED")
    }));
    let stale_ack = json!({"owner": owner, "revision": frame["revision"]});
    assert!(tauri::test::get_ipc_response(&window, request("ack_shell", stale_ack)).is_err());
    invoke(
        &window,
        "ack_shell",
        json!({"owner": current, "revision": frame["revision"]}),
    );
    assert_eq!(
        invoke(
            &window,
            "history_shell",
            json!({"owner": current, "offset": 0})
        )["epoch"],
        epoch
    );

    invoke(&window, "detach_shell", json!({"owner": current}));
    for (command, arguments) in [
        ("open_shell", json!({})),
        ("poll_shell", json!({"owner": current})),
        ("recover_shell", json!({"owner": current})),
        (
            "resize_shell",
            json!({"owner": current, "rows": 10, "cols": 65}),
        ),
        ("write_shell", json!({"owner": current, "bytes": [120]})),
    ] {
        assert!(tauri::test::get_ipc_response(&window, request(command, arguments)).is_err());
    }
    let id = owner["session"].as_str().unwrap();
    let other_id = format!(
        "{}{}",
        if id.starts_with('0') { '1' } else { '0' },
        &id[1..]
    );
    for target in [
        json!({"session": other_id, "epoch": epoch}),
        json!({"session": id, "epoch": epoch + 1}),
        json!({"session": id, "epoch": epoch, "generation": 3}),
    ] {
        assert!(tauri::test::get_ipc_response(
            &window,
            request("attach_shell", json!({"target": target}))
        )
        .is_err());
    }
    let attached = invoke(
        &window,
        "attach_shell",
        json!({"target": {"session": id, "epoch": epoch}}),
    );
    let attached_owner = &attached["owner"];
    assert_eq!(attached_owner["session"], owner["session"]);
    assert_eq!(attached_owner["generation"], 4);
    assert!(tauri::test::get_ipc_response(
        &window,
        request(
            "resize_shell",
            json!({"owner": attached_owner, "rows": 11, "cols": 66})
        ),
    )
    .is_err());
    assert!(tauri::test::get_ipc_response(
        &window,
        request(
            "write_shell",
            json!({"owner": attached_owner, "bytes": [120]})
        ),
    )
    .is_err());
    assert!(
        invoke(&window, "close_shell", json!({"owner": attached_owner}))["cleanup_pending"]
            .is_boolean()
    );
    assert!(tauri::test::get_ipc_response(
        &window,
        request("poll_shell", json!({"owner": current}))
    )
    .is_err());
    let next = invoke(&window, "open_shell", json!({}));
    assert!(next["epoch"].as_u64().unwrap() > epoch);
    assert_ne!(next["session"], owner["session"]);
    assert!(tauri::test::get_ipc_response(
        &window,
        request("close_shell", json!({"owner": attached_owner})),
    )
    .is_err());
    assert_eq!(
        invoke(&window, "shell_status", json!({"owner": next}))["epoch"],
        next["epoch"]
    );
    invoke(&window, "close_shell", json!({"owner": next}));
}

#[test]
fn detached_local_pty_drains_past_kernel_capacity_without_frontend_poll() {
    use std::{
        os::unix::fs::DirBuilderExt,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "aether-local-pressure-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let temp = Scratch(path);
    let script = temp.0.join("pressure.sh");
    let gate = temp.0.join("go");
    let armed = temp.0.join("armed");
    let payload = temp.0.join("payload");
    std::fs::write(&payload, vec![b'X'; 256 * 1024]).unwrap();
    let written = temp.0.join("done");
    fn quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    std::fs::write(
        &script,
        format!(
            r#"original=$(stty -g) || return 1
stty -echo -icanon min 1 time 0 || return 1
printf '%s' "$$" > {armed}
printf '\nAETHER_PRESSURE_ARMED\n'
n=0
while [ ! -f {gate} ] && [ "$n" -lt 300 ]; do sleep .05; n=$((n+1)); done
[ -f {gate} ] || return 1
cat {payload} || return 1
printf '\nAETHER_PRESSURE_END\n\033[H\033[6n' || return 1
# Exact parser reply proves the owner reached the marker before reattach.
reply=$(timeout 4 dd bs=1 count=6 2>/dev/null | od -An -tx1 | tr -d ' \n')
stty "$original" || return 1
[ "$reply" = 1b5b313b3152 ] || return 1
printf '%s' "$$" > {written}
"#,
            gate = quote(&gate),
            armed = quote(&armed),
            payload = quote(&payload),
            written = quote(&written)
        ),
    )
    .unwrap();
    let app = super::application(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let owner = invoke(&window, "open_shell", json!({}));
    invoke(
        &window,
        "text_shell",
        json!({"owner": owner, "text": format!(". {}",quote(&script))}),
    );
    invoke(
        &window,
        "key_shell",
        json!({"owner": owner, "name":"Enter", "shift":false, "alt":false, "ctrl":false}),
    );
    fn marker_rows(rows: &serde_json::Value, expected: &str) -> bool {
        rows.as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row.as_array()
                    .or_else(|| row["cells"].as_array())
                    .is_some_and(|cells| {
                        cells
                            .iter()
                            .filter_map(|cell| cell["text"].as_str())
                            .collect::<String>()
                            .contains(expected)
                    })
            })
        })
    }
    let poll_marker = |owner: &serde_json::Value, label| {
        let update = invoke(&window, "poll_shell", json!({"owner": owner}));
        if update.is_null() {
            return false;
        }
        invoke(
            &window,
            "ack_shell",
            json!({"owner":owner,"revision":update["frame"]["revision"]}),
        );
        marker_rows(&update["frame"]["rows"], label)
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !poll_marker(&owner, "AETHER_PRESSURE_ARMED") {
        assert!(
            Instant::now() < deadline,
            "pressure shell did not reach gate"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let expected_pid = std::fs::read_to_string(&armed).unwrap();
    assert!(!expected_pid.is_empty() && expected_pid.bytes().all(|byte| byte.is_ascii_digit()));
    invoke(&window, "detach_shell", json!({"owner":owner}));
    std::fs::write(&gate, b"go").unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    let completed_without_view = loop {
        // Intentionally no poll, status, attach, callback, or other IPC here.
        if std::fs::read_to_string(&written).is_ok_and(|pid| pid == expected_pid) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let resumed = invoke(
        &window,
        "attach_shell",
        json!({"target":{"session":owner["session"],"epoch":owner["epoch"]}}),
    );
    let attached = &resumed["owner"];
    let end_seen = marker_rows(&resumed["frame"]["rows"], "AETHER_PRESSURE_END");
    let old_ack = json!({"owner":owner,"revision":resumed["frame"]["revision"]});
    assert!(tauri::test::get_ipc_response(&window, request("ack_shell", old_ack)).is_err());
    let readonly = json!({"owner":attached,"bytes":[120]});
    assert!(tauri::test::get_ipc_response(&window, request("write_shell", readonly)).is_err());
    invoke(
        &window,
        "ack_shell",
        json!({"owner":attached,"revision":resumed["frame"]["revision"]}),
    );
    // Retire the owned fixture before failing the negative baseline.
    let closed = invoke(&window, "close_shell", json!({"owner":attached}));
    assert!(closed["cleanup_pending"].is_boolean());
    assert!(invoke(&window, "attach_shell", json!({})).is_null());
    assert_eq!(attached["session"], owner["session"]);
    assert_eq!(attached["epoch"], owner["epoch"]);
    assert_ne!(attached["generation"], owner["generation"]);
    assert!(
        completed_without_view,
        "detached writer stalled without a frontend read"
    );
    assert!(
        end_seen,
        "retained PTY did not return the final pressure marker"
    );
}
