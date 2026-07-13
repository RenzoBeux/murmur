#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use log;
use env_logger;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // `murmur --mcp` runs the built-in read-only MCP server over stdio and
    // never boots the GUI (or the single-instance guard — an MCP client must
    // be able to spawn this while the app is running). stdout is reserved
    // for protocol frames; env_logger writes to stderr.
    if args.iter().any(|a| a == "--mcp") {
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "info");
        }
        env_logger::init();

        let db_path = args
            .iter()
            .position(|a| a == "--db")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var("MURMUR_DB_PATH").ok().map(std::path::PathBuf::from))
            // Legacy env var from before the Meetily -> Murmur rename.
            .or_else(|| std::env::var("MEETILY_DB_PATH").ok().map(std::path::PathBuf::from))
            .map(Ok)
            .unwrap_or_else(app_lib::mcp::default_db_path);

        let db_path = match db_path {
            Ok(p) => p,
            Err(e) => {
                eprintln!("murmur --mcp: could not resolve database path: {e:#}");
                std::process::exit(1);
            }
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");
        if let Err(e) = runtime.block_on(app_lib::mcp::run_stdio(db_path)) {
            eprintln!("murmur --mcp: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    // Default to info, but let the user lower it (e.g. RUST_LOG=warn) instead of
    // forcing info unconditionally in release.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    // Async logger will be initialized lazily when first needed (after Tauri runtime starts)
    log::info!("Starting application...");
    app_lib::run();
}
