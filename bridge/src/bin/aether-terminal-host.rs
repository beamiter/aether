use std::{
    io::{self, Write},
    path::PathBuf,
};
use tauri_terminal_frame_bridge::terminal_transport;

fn run() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let action = args.next();
    if action.as_deref() == Some(std::ffi::OsStr::new("terminfo")) && args.len() == 0 {
        return io::stdout()
            .lock()
            .write_all(jterm_headless_session_spike::terminal_profile::COMPILED_ENTRY);
    }
    if action.as_deref() == Some(std::ffi::OsStr::new("files")) && args.len() == 0 {
        return tauri_terminal_frame_bridge::remote_files::serve(
            io::stdin().lock(),
            io::stdout().lock(),
        );
    }
    let socket = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(terminal_transport::default_socket);
    if args.next().is_none() {
        match action.as_deref().and_then(|value| value.to_str()) {
            Some("serve") => return terminal_transport::serve(&socket),
            Some("stdio") => {
                return terminal_transport::stdio(&socket, io::stdin().lock(), io::stdout().lock())
            }
            _ => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "usage: aether-terminal-host {serve|stdio} [absolute-socket-path] | files | terminfo",
    ))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("aether-terminal-host: {error}");
        std::process::exit(1);
    }
}
