use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let entry = "terminfo/aether.ti";
    println!("cargo:rerun-if-changed={entry}");
    // -I makes tic dump the compiled entry to stdout instead of installing it
    // in a terminfo database. -Q2 selects ncurses' inline base64 representation.
    let output = Command::new("tic")
        .args(["-x", "-I", "-Q2", entry])
        .output()
        .expect("install ncurses-bin for the tic terminfo compiler");
    assert!(
        output.status.success(),
        "tic failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let description = String::from_utf8(output.stdout).expect("tic emits UTF-8");
    let encoded = description
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("b64:"))
        .collect::<String>();
    assert!(
        encoded.starts_with("b64:"),
        "tic did not emit an inline entry"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo build output directory"));
    fs::write(out.join("aether.terminfo"), encoded).expect("save compiled Aether terminfo");
}
