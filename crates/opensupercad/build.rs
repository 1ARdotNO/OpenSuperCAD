//! On Windows, embed the app icon and version details in `opensupercad.exe`,
//! so Explorer, the taskbar and shortcuts show the OpenSuperCAD icon.

fn main() {
    let icon = "../../packaging/windows/opensupercad.ico";
    println!("cargo:rerun-if-changed={icon}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon)
            .set("ProductName", "OpenSuperCAD")
            .set("FileDescription", "OpenSuperCAD");
        if let Err(e) = res.compile() {
            panic!("embedding the Windows icon failed: {e}");
        }
    }
}
