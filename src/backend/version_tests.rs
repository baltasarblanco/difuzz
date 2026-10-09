//! B3 (R1-H11): `Minifier::version()` contra `Cargo.lock`.
//!
//! `version()` es un string escrito a mano que va a cada `meta.txt`, a
//! `CORRIDA.txt` y a la validacion del oraculo (`subproceso.rs`).
//! `swc_ecma_minifier` va con caret en `Cargo.toml`: un `cargo update` lo puede
//! mover sin que nadie toque el string, y los hallazgos quedarian etiquetados
//! con una version que no es la que se midio. Este test lee el `Cargo.lock`
//! con el que se compila el crate y falla si no coincide.
//!
//! `include_str!` hace que cargo registre el lock como dependencia del test:
//! si el lock cambia, el test se recompila y vuelve a leerlo.

use super::Minifier;

/// Las versiones de `krate` en un `Cargo.lock` (puede haber mas de una si dos
/// dependencias piden versiones incompatibles).
fn versiones_en_lock(lock: &str, krate: &str) -> Vec<String> {
    lock.split("[[package]]")
        .filter_map(|bloque| {
            let mut nombre = None;
            let mut version = None;
            for l in bloque.lines().map(str::trim) {
                if let Some(v) = l.strip_prefix("name = ") {
                    nombre = Some(v.trim_matches('"'));
                } else if let Some(v) = l.strip_prefix("version = ") {
                    version = Some(v.trim_matches('"'));
                }
            }
            if nombre == Some(krate) {
                version.map(str::to_string)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn b3_version_coincide_con_cargo_lock() {
    let lock = include_str!("../../Cargo.lock");
    for m in [Minifier::Swc, Minifier::Oxc] {
        let (krate, ver) = m
            .version()
            .split_once(' ')
            .unwrap_or_else(|| panic!("{:?}: version() no es \"<crate> <version>\": {:?}", m, m.version()));
        let en_lock = versiones_en_lock(lock, krate);
        assert_eq!(
            en_lock,
            [ver],
            "{}: version() dice {krate} {ver} y Cargo.lock tiene {krate} {en_lock:?}; actualizar Minifier::version() (src/backend/mod.rs)",
            m.name()
        );
    }
}

/// El lector del lock: ignora la cabecera, las listas de dependencias (que
/// tambien nombran crates con su version) y distingue nombres con prefijo comun.
#[test]
fn versiones_en_lock_lee_el_formato() {
    let lock = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n \"ab 2.0.0\",\n]\n\n[[package]]\nname = \"ab\"\nversion = \"2.0.0\"\n\n[[package]]\nname = \"ab\"\nversion = \"3.1.0\"\n";
    assert_eq!(versiones_en_lock(lock, "a"), ["1.0.0"]);
    assert_eq!(versiones_en_lock(lock, "ab"), ["2.0.0", "3.1.0"]);
    assert!(versiones_en_lock(lock, "b").is_empty());
}
