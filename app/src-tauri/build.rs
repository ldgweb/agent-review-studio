fn main() {
    // The `ars` CLI ships as a Tauri sidecar (externalBin), and tauri-build
    // refuses to compile when the staged binary is missing. Creating an empty
    // placeholder keeps plain `cargo build` / `cargo test` working without a
    // staging step; scripts/stage-cli.mjs overwrites it with the real binary
    // before anything is actually bundled (tauri:dev / tauri:build / CI).
    let triple = std::env::var("TARGET").unwrap_or_default();
    let ext = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let dir = std::path::Path::new("binaries");
    let placeholder = dir.join(format!("ars-{triple}{ext}"));
    if !placeholder.exists() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&placeholder, b"");
    }

    tauri_build::build()
}
