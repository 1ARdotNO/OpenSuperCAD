//! Agent registry location and loading.

use std::path::PathBuf;

use osc_agent::Registry;

/// Where `agents.json` lives: `~/.config/opensupercad` on Linux,
/// `~/Library/Application Support/OpenSuperCAD` on macOS.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| {
            d.join(if cfg!(target_os = "macos") {
                "OpenSuperCAD"
            } else {
                "opensupercad"
            })
        })
        .unwrap_or_else(|| PathBuf::from(".opensupercad-config"))
}

/// Built-in agents plus user overrides. A broken `agents.json` is reported
/// on stderr and ignored rather than preventing startup.
pub fn registry() -> Registry {
    Registry::load(&config_dir()).unwrap_or_else(|e| {
        eprintln!("ignoring invalid agents.json: {e}");
        Registry::default()
    })
}
