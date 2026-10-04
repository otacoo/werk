//! MSVC only: embed the comctl32 v6 manifest and mirror tauri-build's static
//! vcruntime setup. The tool links the Tauri app lib, whose imports need both;
//! `tauri-build` adds these for the app package, but this crate lives outside
//! it.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    // The app lib's Windows deps emit C++ exception-handling shims; resolve
    // them against the static C++ runtime (the app's msvcrt stub shadows the
    // import lib through the inherited link search path).
    for lib in [
        "libvcruntimed.lib",
        "vcruntime.lib",
        "vcruntimed.lib",
        "libcmtd.lib",
        "msvcrt.lib",
        "msvcrtd.lib",
        "libucrt.lib",
        "libucrtd.lib",
    ] {
        println!("cargo:rustc-link-arg-bins=/NODEFAULTLIB:{lib}");
    }
    for lib in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg-bins=/DEFAULTLIB:{lib}");
    }
}
