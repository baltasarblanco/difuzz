//! GEN: el generador mas tonto posible. Archivos `input.js` del corpus de SWC,
//! sin mutar. La interfaz es un Vec de rutas: otro generador puede ocupar su lugar.

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
        // M27 (e): `file_type` no sigue symlinks: un link a un directorio (por
        // ejemplo a un padre) no se recorre, y no hay ciclo hasta ELOOP.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&path, out);
        } else if path.file_name().is_some_and(|n| n == "input.js") {
            out.push(path);
        }
    }
}

/// M27 (d): por que `root` no sirve de corpus, con un mensaje que lo diga.
pub fn check_root(root: &Path) -> Result<(), String> {
    match std::fs::metadata(root) {
        Err(e) => Err(format!("el corpus {} no existe o no se puede leer: {e}", root.display())),
        Ok(m) if !m.is_dir() => Err(format!("el corpus {} no es un directorio", root.display())),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    /// M27 (e): un symlink que cicla no duplica casos.
    #[test]
    fn no_sigue_symlinks_a_directorios() {
        let d = std::env::temp_dir().join(format!("difuzz-test-corpus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::write(d.join("a/input.js"), "1;").unwrap();
        std::os::unix::fs::symlink(&d, d.join("a/vuelta")).unwrap();
        assert_eq!(super::collect(&d), [d.join("a/input.js")]);
        assert!(super::check_root(&d).is_ok());
        assert!(super::check_root(&d.join("a/input.js")).unwrap_err().contains("no es un directorio"));
        assert!(super::check_root(&d.join("no")).unwrap_err().contains("no existe"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
