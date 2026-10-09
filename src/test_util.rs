//! R1-H17: utilidades comunes de los tests.
//!
//! - `requiere_node()`: un test que necesita node FALLA si no hay node en el
//!   PATH. Con `DIFUZZ_SIN_NODE=1` lo saltea y lo deja contado en stderr con el
//!   prefijo `DIFUZZ_SALTEADO_SIN_NODE` al principio de la linea (contar con
//!   `cargo test --no-fail-fast 2>&1 | rg -c '^DIFUZZ_SALTEADO_SIN_NODE'`).
//! - `id_runner()`: el id de `Runner` de cada test. Dos `Runner` con el mismo id
//!   en el mismo proceso comparten script, ctl y cwd, y en paralelo uno pisa al
//!   otro (paso con 9988 y 9989). El test `ningun_runner_con_id_fijo` falla si
//!   un `Runner::new(` de `src/` o `tests/` lleva un id literal.
//!
//! No depende de nada del crate: `tests/salida_celdas.rs` lo incluye con
//! `#[path]` (difuzz es solo binario, no hay `difuzz::` para importar).

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Variable que permite saltear (y contar) los tests que necesitan node.
pub const SIN_NODE: &str = "DIFUZZ_SIN_NODE";

/// Prefijo de la linea que deja cada test salteado.
pub const PREFIJO_SALTEADO: &str = "DIFUZZ_SALTEADO_SIN_NODE";

static SALTEADOS: AtomicUsize = AtomicUsize::new(0);

fn hay_node() -> bool {
    static HAY: OnceLock<bool> = OnceLock::new();
    *HAY.get_or_init(|| {
        std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
    })
}

/// `true` si hay node (el test sigue). Sin node: con `DIFUZZ_SIN_NODE=1`
/// devuelve `false` (el test hace `return`) y lo anota en stderr; sin la
/// variable, panic con un mensaje claro.
pub fn requiere_node() -> bool {
    if hay_node() {
        return true;
    }
    let test = std::thread::current().name().unwrap_or("?").to_string();
    if std::env::var(SIN_NODE).as_deref() != Ok("1") {
        panic!(
            "{test}: este test necesita node y no hay node en el PATH. \
             Instala node o, para saltear a sabiendas los tests que lo necesitan, \
             defini {SIN_NODE}=1 (cada salteo deja una linea {PREFIJO_SALTEADO} en stderr)."
        );
    }
    let n = SALTEADOS.fetch_add(1, Ordering::SeqCst) + 1;
    // Directo a stderr y no con eprintln!: libtest captura lo de eprintln! y, si
    // el test pasa, no lo muestra. Una sola escritura y con salto de linea
    // adelante: libtest puede tener a medias su linea `test ... ` en stdout.
    let linea = format!("\n{PREFIJO_SALTEADO}: {test} (salteados en este binario: {n})\n");
    let _ = std::io::stderr().write_all(linea.as_bytes());
    false
}

/// Base de los ids de test: lejos de los ids de produccion (`0..jobs`).
const BASE_ID_RUNNER: usize = 1_000_000;

static PROXIMO_ID: AtomicUsize = AtomicUsize::new(BASE_ID_RUNNER);

/// Un id de `Runner` que ningun otro test del proceso recibe.
pub fn id_runner() -> usize {
    PROXIMO_ID.fetch_add(1, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn fuentes(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                fuentes(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Los ids literales de los `Runner::new(` de una linea: el segundo
    /// argumento, hasta la coma siguiente, si es un numero.
    fn ids_en_linea(linea: &str) -> Vec<String> {
        let aguja = concat!("Runner", "::new(");
        let mut ids = Vec::new();
        let mut resto = linea;
        while let Some(pos) = resto.find(aguja) {
            resto = &resto[pos + aguja.len()..];
            let arg = resto.split(',').nth(1).unwrap_or("").trim();
            let digitos = arg.trim_end_matches("usize").replace('_', "");
            if !digitos.is_empty() && digitos.chars().all(|c| c.is_ascii_digit()) {
                ids.push(arg.to_string());
            }
        }
        ids
    }

    /// Ids de `Runner` literales en `src/` y `tests/`: (archivo:linea, id).
    fn ids_literales() -> Vec<(String, String)> {
        let raiz = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut archivos = Vec::new();
        fuentes(&raiz.join("src"), &mut archivos);
        fuentes(&raiz.join("tests"), &mut archivos);
        let mut hallados = Vec::new();
        for f in archivos {
            let txt = std::fs::read_to_string(&f).unwrap();
            for (i, linea) in txt.lines().enumerate() {
                for id in ids_en_linea(linea) {
                    let rel = f.strip_prefix(raiz).unwrap_or(&f).display().to_string();
                    hallados.push((format!("{rel}:{}", i + 1), id));
                }
            }
        }
        hallados
    }

    /// Ningun test usa un id de `Runner` fijo: todos salen de `id_runner()`.
    /// Un id literal es la forma de que dos tests compartan cwd (y si se
    /// repite, el test lo dice).
    #[test]
    fn ningun_runner_con_id_fijo() {
        let hallados = ids_literales();
        let mut repetidos: Vec<&str> = Vec::new();
        for (i, (_, id)) in hallados.iter().enumerate() {
            if hallados[..i].iter().any(|(_, otro)| otro == id) && !repetidos.contains(&id.as_str()) {
                repetidos.push(id);
            }
        }
        assert!(
            hallados.is_empty(),
            "Runner::new con id literal (usar crate::test_util::id_runner()); repetidos: {repetidos:?}\n{}",
            hallados.iter().map(|(donde, id)| format!("  {donde}: {id}")).collect::<Vec<_>>().join("\n")
        );
    }

    /// El detector ve un id literal (si no, el test de arriba no prueba nada)
    /// y no confunde el id de produccion (`w`) ni `id_runner()` con uno.
    #[test]
    fn el_detector_ve_ids_literales() {
        let new = concat!("Runner", "::new(");
        assert_eq!(ids_en_linea(&format!("let r = {new}&preload, 9988, Duration::from_secs(20));")), ["9988"]);
        assert_eq!(ids_en_linea(&format!("let r = runner::{new}&preload, 1_000usize, d);")), ["1_000usize"]);
        assert!(ids_en_linea(&format!("let runner = runner::{new}preload, w, cli.timeout)")).is_empty());
        assert!(ids_en_linea(&format!("let r = {new}&preload, crate::test_util::id_runner(), d);")).is_empty());
    }

    /// `id_runner()` no repite ids aunque lo pidan muchos hilos a la vez.
    #[test]
    fn id_runner_no_repite() {
        let hilos: Vec<_> = (0..8).map(|_| std::thread::spawn(|| (0..100).map(|_| super::id_runner()).collect::<Vec<_>>())).collect();
        let mut ids: Vec<usize> = hilos.into_iter().flat_map(|h| h.join().unwrap()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(ids[0] >= super::BASE_ID_RUNNER);
    }
}
