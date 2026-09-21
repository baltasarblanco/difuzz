//! GEN, version F1: el generador mas tonto posible. Archivos `input.js` del
//! corpus de SWC, sin mutar. Se reemplaza en F2/F3; la interfaz es un Vec de rutas.

use std::path::{Path, PathBuf};

/// Todos los `input.js` bajo `root`, ordenados: el mismo corpus recorre igual
/// en cada corrida.
pub fn collect(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.file_name().is_some_and(|n| n == "input.js") {
            out.push(path);
        }
    }
}
