mod application;

fn main() {
    application::configure(tauri::Builder::default(), application::Workspace::default())
        .run(tauri::generate_context!())
        .expect("Tauri runtime");
}

#[cfg(test)]
mod ipc_tests;
