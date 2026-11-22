#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HOST: &str = env!("CARGO_BIN_EXE_aether-terminal-host");

struct PrivateHost {
    directory: PathBuf,
    child: Child,
}

impl PrivateHost {
    fn open() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("aether-drain-{}-{unique}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let child = Command::new(HOST)
            .arg("serve")
            .arg(directory.join("host.sock"))
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let host = Self { directory, child };
        wait_for("remote owner did not bind", || {
            fs::metadata(host.socket())
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
        });
        host
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("host.sock")
    }

    // A fresh stdio relay runs for exactly one request and leaves no
    // connection behind. Between calls only the remote owner and PTY live.
    fn request(&self, command: Value) -> Value {
        let mut relay = Command::new(HOST)
            .arg("stdio")
            .arg(self.socket())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = relay.stdin.take().unwrap();
        serde_json::to_writer(
            &mut stdin,
            &json!({"protocol":1,"request":22,"command":command}),
        )
        .unwrap();
        stdin.write_all(b"\n").unwrap();
        drop(stdin);
        let output = relay.wait_with_output().unwrap();
        assert!(output.status.success(), "{:?}", output.status);
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["request"], 22);
        assert!(reply["result"]["Err"].is_null(), "{reply}");
        reply["result"]["Ok"].clone()
    }
}

impl Drop for PrivateHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn wait_for(label: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < deadline, "{label}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
fn retained_remote_host_drains_real_pty_pressure_between_relays() {
    let host = PrivateHost::open();
    let owner = host.request(json!({"kind":"open","rows":24,"cols":80}));
    let receipt = host.directory.join("finished");
    let command = format!(
        "stty -echo; head -c 524288 /dev/zero | tr '\\000' x; printf '\\r\\nREMOTE_DRAIN_TAIL\\r\\n'; printf '%s' \"$$\" > {}; exit 7\n",
        shell_path(&receipt)
    );
    let written = host.request(json!({"kind":"write","owner":owner,"bytes":command.as_bytes()}));
    assert_eq!(written, command.len());

    wait_for("remote PTY blocked with no relay or frame poll", || {
        fs::metadata(&receipt).is_ok_and(|metadata| metadata.len() > 0)
    });
    wait_for("remote owner did not observe child exit and EOF", || {
        host.request(json!({"kind":"status","owner":owner}))["drained"] == true
    });
    let recovered = host.request(json!({"kind":"recover","owner":owner}));
    assert_eq!(owner["session"], recovered["owner"]["session"]);
    assert_eq!(owner["epoch"], recovered["owner"]["epoch"]);
    assert_ne!(owner["generation"], recovered["owner"]["generation"]);
    let current = &recovered["owner"];
    let frame = &recovered["frame"];
    assert!(frame["rows"].as_array().unwrap().iter().any(|row| {
        row.as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["text"].as_str().unwrap())
            .collect::<String>()
            .contains("REMOTE_DRAIN_TAIL")
    }));
    host.request(json!({"kind":"ack","owner":current,"revision":frame["revision"]}));
    host.request(json!({"kind":"close","owner":current}));
}
