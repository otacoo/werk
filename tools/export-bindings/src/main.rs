//! Regenerates `src/bindings.ts` from the app's canonical command list.
//! Lives outside the Tauri package on purpose: extra bins there get built by
//! `cargo build --bins` and then break bundling on universal macOS targets.
//! CI runs this, then fails on a dirty tree if bindings went stale.

use specta_typescript::Typescript;
use werk_lib::bindings_builder;

fn main() {
    // Absolute: relative paths resolve against the process CWD, which
    // differs between `cargo run`, the dev server, and the built app.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/bindings.ts");
    bindings_builder()
        .export(Typescript::default(), path)
        .expect("export typescript bindings");
    println!("exported src/bindings.ts");
}
