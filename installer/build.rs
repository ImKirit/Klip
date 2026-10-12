//! Packs everything under `dist-stage/` into the setup program. The folder is filled by
//! `scripts/stage.ps1`. A missing folder gives a setup without payload, which still builds
//! so the workspace compiles on a fresh checkout.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, base, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

fn main() {
    let stage = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("dist-stage");
    println!("cargo:rerun-if-changed={}", stage.display());
    let mut files = Vec::new();
    collect(&stage, &stage, &mut files);
    files.sort();

    let mut code = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let _ = writeln!(code, "    ({rel:?}, include_bytes!({:?})),", path.display().to_string());
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("files.rs");
    std::fs::write(out, code).unwrap();
}
