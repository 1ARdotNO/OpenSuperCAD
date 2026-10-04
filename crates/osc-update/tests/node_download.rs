//! Downloads the pinned Node.js for this platform, installs it like the app
//! does and runs `node` and `npx` from it. Ignored by default (it fetches
//! 35-60 MB); the `OpenSCAD pins` workflow runs it on Linux, macOS and Windows.

use std::process::Command;

use osc_update::Client;
use osc_update::node::{build_for_this_platform, install, installed, version};

#[test]
#[ignore = "downloads the pinned Node.js build (35-60 MB)"]
fn pinned_node_installs_and_runs() {
    let Some(build) = build_for_this_platform() else {
        eprintln!("no pinned Node.js for this platform; nothing to check");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let bin = install(&Client::default(), &build, home.path(), |_| {}).expect("install Node.js");
    assert_eq!(installed(home.path()), Some(bin.clone()));

    // npx is a script that runs `node` from PATH, as the agents do.
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let npx = bin.join(if cfg!(windows) { "npx.cmd" } else { "npx" });
    let out = Command::new(&npx)
        .arg("--version")
        .env("PATH", &path)
        .output()
        .expect("run npx");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let node = bin.join(if cfg!(windows) { "node.exe" } else { "node" });
    let out = Command::new(node).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("v{}", version())
    );
}
