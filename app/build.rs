fn main() {
    // Record the dev shell's library and tool paths, so `invisible-ai <command>` can
    // start the app from outside it (e.g. from a compositor hotkey).
    for var in ["LD_LIBRARY_PATH", "PATH"] {
        println!("cargo:rerun-if-env-changed={var}");
        println!("cargo:rustc-env=BUILD_{var}={}", std::env::var(var).unwrap_or_default());
    }
    tauri_build::build()
}
