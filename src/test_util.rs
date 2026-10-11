//! R1-H17: utilidades comunes de los tests.
//!
//! - `requiere_node()`: un test que necesita node FALLA si no hay node en el
//!   PATH. Con `DIFUZZ_SIN_NODE=1` lo saltea y lo deja contado en stderr con el
//!   prefijo `DIFUZZ_SALTEADO_SIN_NODE` al principio de la linea (contar con
//!   `cargo test --no-fail-fast 2>&1 | rg -c '^DIFUZZ_SALTEADO_SIN_NODE'`).
//! - `requiere_userns(prueba)` (7.5): lo mismo para los tests de `--sandbox-red`, que
//!   necesitan user namespaces. `prueba` es el resultado de
//!   `runner::probar_sandbox(Sandbox::PermisosYRed)` (este archivo no depende del crate).
//!   Si fallo, el test FALLA; con `DIFUZZ_SIN_USERNS=1` lo saltea y lo cuenta con el
//!   prefijo `DIFUZZ_SALTEADO_SIN_USERNS`.
//! - `id_runner()`: el id de `Runner` de cada test. Dos `Runner` con el mismo id
//!   en el mismo proceso comparten script, ctl y cwd, y en paralelo uno pisa al
//!   otro (paso con 9988 y 9989). El test `ningun_runner_con_id_fijo` falla si
//!   un `Runner::new(` de `src/` o `tests/` lleva un id fijo (literal, constante o
//!   `let` a un literal; M4: tambien con la llamada partida en varias lineas).
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

/// 7.5: variable que permite saltear (y contar) los tests que necesitan user namespaces.
pub const SIN_USERNS: &str = "DIFUZZ_SIN_USERNS";

/// 7.5: prefijo de la linea que deja cada test salteado sin user namespaces.
pub const PREFIJO_SALTEADO_USERNS: &str = "DIFUZZ_SALTEADO_SIN_USERNS";

static SALTEADOS_USERNS: AtomicUsize = AtomicUsize::new(0);

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
    avisar_salteo(PREFIJO_SALTEADO, &test, n);
    false
}

/// 7.5: `true` si esta maquina arma el sandbox con red (`prueba` es `Ok`; el test sigue).
/// Si no: con `DIFUZZ_SIN_USERNS=1` devuelve `false` (el test saltea esa parte) y lo
/// anota en stderr; sin la variable, panic con el error de `prueba`. Antes estos tests
/// pasaban sin probar nada si no habia user namespaces.
pub fn requiere_userns(prueba: Result<(), String>) -> bool {
    let Err(e) = prueba else {
        return true;
    };
    let test = std::thread::current().name().unwrap_or("?").to_string();
    if std::env::var(SIN_USERNS).as_deref() != Ok("1") {
        panic!(
            "{test}: este test necesita el sandbox con red (user namespaces) y esta maquina no lo arma: {e}. \
             Para saltear a sabiendas lo que lo necesita, defini {SIN_USERNS}=1 \
             (cada salteo deja una linea {PREFIJO_SALTEADO_USERNS} en stderr)."
        );
    }
    let n = SALTEADOS_USERNS.fetch_add(1, Ordering::SeqCst) + 1;
    avisar_salteo(PREFIJO_SALTEADO_USERNS, &format!("{test}: {e}"), n);
    false
}

fn avisar_salteo(prefijo: &str, que: &str, n: usize) {
    // Directo a stderr y no con eprintln!: libtest captura lo de eprintln! y, si
    // el test pasa, no lo muestra. Una sola escritura y con salto de linea
    // adelante: libtest puede tener a medias su linea `test ... ` en stdout.
    let linea = format!("\n{prefijo}: {que} (salteados en este binario: {n})\n");
    let _ = std::io::stderr().write_all(linea.as_bytes());
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

    /// M4 (revision-b456): el segundo argumento de cada `Runner::new(` de un fuente,
    /// con su linea, aunque la llamada este partida en varias lineas (se lee hasta el
    /// `)` que la cierra, contando parentesis, corchetes y llaves). Se saltean las
    /// apariciones dentro de un comentario `//` de la misma linea.
    fn segundos_args(txt: &str) -> Vec<(usize, String)> {
        let aguja = concat!("Runner", "::new(");
        let mut v = Vec::new();
        let mut desde = 0;
        while let Some(pos) = txt[desde..].find(aguja) {
            let pos = desde + pos;
            desde = pos + aguja.len();
            let ini_linea = txt[..pos].rfind('\n').map_or(0, |i| i + 1);
            if txt[ini_linea..pos].contains("//") {
                continue;
            }
            let mut prof = 0usize;
            let mut args = vec![String::new()];
            for c in txt[desde..].chars() {
                match c {
                    '(' | '[' | '{' => prof += 1,
                    ')' | ']' | '}' if prof == 0 => break,
                    ')' | ']' | '}' => prof -= 1,
                    ',' if prof == 0 => {
                        args.push(String::new());
                        continue;
                    }
                    _ => {}
                }
                args.last_mut().expect("nunca vacio").push(c);
            }
            if let Some(a) = args.get(1) {
                v.push((txt[..pos].matches('\n').count() + 1, a.split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        }
        v
    }

    /// M4: una expresion que da un id fijo: tiene un literal numerico (`9988`,
    /// `1_000usize`, `9000 + i`) o una constante (identificador en mayusculas:
    /// `ID_FIJO`, `otro::ID`, `usize::MAX`).
    fn es_fija(expr: &str) -> bool {
        expr.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|t| !t.is_empty()).any(|t| {
            t.starts_with(|c: char| c.is_ascii_digit())
                || (t.starts_with(|c: char| c.is_ascii_uppercase()) && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        })
    }

    /// M4: los ids fijos de los `Runner::new(` de un fuente: (linea, id). Tambien un
    /// identificador que el mismo fuente liga con `let` a una expresion fija
    /// (`let id = 9988;`); si el nombre se liga en otro lado a algo fijo, lo marca
    /// igual (conservador: lo revisa una persona).
    fn ids_fijos(txt: &str) -> Vec<(usize, String)> {
        let es_ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let ligado_fijo = |id: &str| {
            txt.match_indices(&format!("let {id}")).any(|(i, m)| {
                let resto = &txt[i + m.len()..];
                let (sigue, rhs) = (resto.chars().next(), resto.split(';').next().unwrap_or(""));
                matches!(sigue, Some(' ' | ':' | '=')) && rhs.split_once('=').is_some_and(|(_, e)| es_fija(e))
            })
        };
        segundos_args(txt).into_iter().filter(|(_, a)| es_fija(a) || (es_ident(a) && ligado_fijo(a))).collect()
    }

    /// Ids de `Runner` fijos en `src/` y `tests/`: (archivo:linea, id).
    fn ids_literales() -> Vec<(String, String)> {
        let raiz = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut archivos = Vec::new();
        fuentes(&raiz.join("src"), &mut archivos);
        fuentes(&raiz.join("tests"), &mut archivos);
        let mut hallados = Vec::new();
        for f in archivos {
            let txt = std::fs::read_to_string(&f).unwrap();
            for (linea, id) in ids_fijos(&txt) {
                let rel = f.strip_prefix(raiz).unwrap_or(&f).display().to_string();
                hallados.push((format!("{rel}:{linea}"), id));
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
    /// y no confunde el id de produccion (`w`) ni `id_runner()` con uno. M4: tambien
    /// llamadas partidas en varias lineas, constantes, expresiones y `let` a un literal.
    #[test]
    fn el_detector_ve_ids_literales() {
        let new = concat!("Runner", "::new(");
        let ids = |s: String| ids_fijos(&s).into_iter().map(|(l, id)| format!("{l}:{id}")).collect::<Vec<_>>();
        assert_eq!(ids(format!("let r = {new}&preload, 9988, Duration::from_secs(20));")), ["1:9988"]);
        assert_eq!(ids(format!("let r = runner::{new}&preload, 1_000usize, d);")), ["1:1_000usize"]);
        assert!(ids(format!("let runner = runner::{new}preload, w, cli.timeout)")).is_empty());
        assert!(ids(format!("let r = {new}&preload, crate::test_util::id_runner(), d);")).is_empty());
        // Partida en varias lineas (lo que `cargo fmt` hace con una llamada larga).
        assert_eq!(ids(format!("x;\nlet r = {new}\n    &preload,\n    9988,\n    Duration::from_secs(20),\n);")), ["2:9988"]);
        assert!(ids(format!("let r = {new}\n    &preload,\n    crate::test_util::id_runner(),\n    d,\n);")).is_empty());
        // Constantes con nombre, expresiones y `let` ligado a algo fijo.
        assert_eq!(ids(format!("const ID_FIJO: usize = 7;\nlet r = {new}&p, ID_FIJO, d);")), ["2:ID_FIJO"]);
        assert_eq!(ids(format!("let r = {new}&p, otro::ID, d);")), ["1:otro::ID"]);
        assert_eq!(ids(format!("let r = {new}&p, 9000 + i, d);")), ["1:9000 + i"]);
        assert_eq!(ids(format!("let id = 9988;\nlet r = {new}&p, id, d);")), ["2:id"]);
        assert_eq!(ids(format!("let id: usize = BASE + 1;\nlet r = {new}&p, id, d);")), ["2:id"]);
        assert!(ids(format!("let id = crate::test_util::id_runner();\nlet r = {new}&p, id, d);")).is_empty());
        // Argumentos con parentesis y comas adentro; y un comentario no cuenta.
        assert!(ids(format!("let r = {new}&f(a, 1), g(), d);")).is_empty());
        assert!(ids(format!("// un `{new}` de `src/` con id 9988, d)\nx;")).is_empty());
    }

    /// 7.5: con el sandbox con red, sigue; sin el, falla salvo `DIFUZZ_SIN_USERNS=1`, que
    /// saltea (y no "pasa probando").
    #[test]
    fn requiere_userns_falla_sin_la_variable() {
        assert!(super::requiere_userns(Ok(())));
        let r = std::panic::catch_unwind(|| super::requiere_userns(Err("prueba: sin user namespaces".into())));
        if std::env::var(super::SIN_USERNS).as_deref() == Ok("1") {
            assert_eq!(r.ok(), Some(false));
        } else {
            assert!(r.is_err(), "sin {} tenia que fallar", super::SIN_USERNS);
        }
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
