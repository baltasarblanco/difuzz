//! Las verificaciones de deteccion como tests de cargo, ignorados por defecto
//! (necesitan node y corren difuzz sobre corpus chicos en varias celdas):
//!
//! - `golden`: `tests/golden/verificar.sh` (bugs conocidos en 8 celdas y modos opt-in).
//! - `aceptacion_rechequeos`: `tests/aceptacion/rechequeos/verificar.sh` (re-chequeos, 4 celdas de oxc).
//!
//! Se corren con `cargo test --release --test verificaciones -- --ignored` (o filtrando por
//! nombre: `-- --ignored golden`). Usan el binario de `CARGO_BIN_EXE_difuzz` y exigen el
//! oraculo sin debug assertions (`cargo build --profile oraculo`, o `DIFUZZ_ORACULO=RUTA`):
//! lo esperado lo asume. Ver `tests/README.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

// R1-H17: el mismo `requiere_node()` que el resto de los tests (difuzz es solo binario).
#[allow(dead_code)]
#[path = "../src/test_util.rs"]
mod test_util;

const BIN: &str = env!("CARGO_BIN_EXE_difuzz");

/// Los mismos candidatos que `subproceso::candidatos_oraculo`: `DIFUZZ_ORACULO` si
/// esta (no vacia), si no `<bin>-oraculo` y `<dir de bin>/../oraculo/difuzz`.
fn oraculo() -> Option<PathBuf> {
    if let Some(e) = std::env::var_os("DIFUZZ_ORACULO").filter(|e| !e.is_empty()) {
        let p = PathBuf::from(e);
        return p.is_file().then_some(p);
    }
    let bin = Path::new(BIN);
    let mut hermano = bin.as_os_str().to_owned();
    hermano.push("-oraculo");
    let mut v = vec![PathBuf::from(hermano)];
    if let (Some(dir), Some(nombre)) = (bin.parent().and_then(|d| d.parent()), bin.file_name()) {
        v.push(dir.join("oraculo").join(nombre));
    }
    v.into_iter().find(|p| p.is_file())
}

fn verificar(script: &str) {
    // Corre difuzz de punta a punta: sin node falla (salvo `DIFUZZ_SIN_NODE=1`).
    if !test_util::requiere_node() {
        return;
    }
    // Que exista no alcanza (difuzz lo valida con --preparar --info); si no sirve, difuzz
    // avisa y verificar.sh lo repite al final de su salida.
    assert!(
        oraculo().is_some(),
        "no encuentro el oraculo sin debug assertions para {BIN}: compilalo con \
         `cargo build --profile oraculo` (queda en target/oraculo/difuzz) o pasalo con DIFUZZ_ORACULO=RUTA"
    );
    let raiz = Path::new(env!("CARGO_MANIFEST_DIR"));
    let o = Command::new("bash").arg(raiz.join(script)).arg(BIN).output().expect("no pude correr bash");
    let salida = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{script} salio con {:?}\n{salida}\n{err}", o.status);
    // Una linea final "comparaciones=N fallas=0 ..." con N > 0.
    let ultima = salida.lines().rev().find(|l| l.starts_with("comparaciones=")).unwrap_or("");
    assert!(ultima.contains(" fallas=0 ") && !ultima.starts_with("comparaciones=0 "), "{salida}");
    eprintln!("{script}: {ultima}");
}

#[test]
#[ignore = "necesita node y el oraculo; correr con -- --ignored"]
fn golden() {
    verificar("tests/golden/verificar.sh");
}

#[test]
#[ignore = "necesita node y el oraculo; correr con -- --ignored"]
fn aceptacion_rechequeos() {
    verificar("tests/aceptacion/rechequeos/verificar.sh");
}
