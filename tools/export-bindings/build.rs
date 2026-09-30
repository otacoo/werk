//! MSVC only: embed the comctl32 v6 manifest. The tool links the Tauri app
//! lib, whose imports need it — `tauri-build` used to add this for every bin
//! in the app package, but this crate lives outside it.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
}
