// Embeds the app icon and version information into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/shadersweep.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    // GPUI loads the window and taskbar icon from resource ID 1.
    resource
        .set_icon_with_id("assets/shadersweep.ico", "1")
        .set("ProductName", "Shadersweep")
        .set("FileDescription", "Shadersweep")
        .set("LegalCopyright", "etherloper");
    if let Err(err) = resource.compile() {
        panic!("failed to embed Windows resources: {err}");
    }
}
