//! Writes the keyboard geometry to `layouts/qwertz.json` and `layouts/qwerty.json`, so
//! the app can draw the boards without calling Rust. A unit test fails when these files
//! no longer match the code.
//!
//! cargo run -p klip-input --example export_layouts

use klip_input::KeyboardLayout;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("layouts");
    std::fs::create_dir_all(&dir).expect("create layouts folder");
    for (name, layout) in [("qwertz.json", KeyboardLayout::iso_qwertz()), ("qwerty.json", KeyboardLayout::ansi_qwerty())] {
        let path = dir.join(name);
        std::fs::write(&path, layout.to_json()).expect("write layout");
        println!("{} ({} keys)", path.display(), layout.keys.len());
    }
}
