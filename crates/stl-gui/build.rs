// The program icon (crates/stl-gui/assets/stellaris-launcher.ico, drawn by tools/make_icon.py) goes into the exe as a Windows resource, so
// Explorer, the taskbar and shortcuts show it.
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=../stl-gui/assets/stellaris-launcher.ico");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
