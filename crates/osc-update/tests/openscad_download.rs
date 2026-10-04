//! Downloads the pinned OpenSCAD for this platform, installs it like the app
//! does and renders a cube with it. Ignored by default (it fetches 50-85 MB);
//! the `OpenSCAD pins` workflow runs it on Linux, macOS and Windows.

use osc_engine::{Engine, Request};
use osc_update::Client;
use osc_update::openscad::{build_for_this_platform, install};

#[test]
#[ignore = "downloads the pinned OpenSCAD build (50-85 MB)"]
fn pinned_build_installs_and_renders() {
    let Some(build) = build_for_this_platform() else {
        eprintln!("no pinned OpenSCAD for this platform; nothing to check");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let mut last = 0;
    let binary = install(&Client::default(), &build, home.path(), |p| {
        if let osc_update::openscad::Progress::Downloading { received, total } = p
            && received * 10 / total.max(1) > last
        {
            last = received * 10 / total.max(1);
            eprintln!("downloaded {}%", last * 10);
        }
    })
    .expect("install the pinned build");

    let engine = Engine::new(&binary);
    let version = engine.version().unwrap();
    eprintln!("{version} at {}", binary.display());
    assert!(version.contains(&build.version), "{version}");

    let dir = tempfile::tempdir().unwrap();
    let scad = dir.path().join("cube.scad");
    std::fs::write(&scad, "cube(10);\n").unwrap();
    let out = dir.path().join("cube.stl");
    let result = engine.export(&Request::new(&scad), &out).unwrap();
    assert!(result.success, "{}", result.console);
    assert!(out.metadata().unwrap().len() > 0);

    // What the app and the MCP server will find.
    let found = Engine::discover_in(home.path()).unwrap();
    if std::env::var_os("OPENSUPERCAD_OPENSCAD").is_none() {
        assert_eq!(found.binary, binary);
    }
}
