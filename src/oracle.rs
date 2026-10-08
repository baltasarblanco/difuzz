//! CMP: que cuenta como divergencia (delta v1.2 §1 §3.4) y como se guarda.
//!
//! Precondicion que garantiza main: el original termino solo (exit 0, o lanzando
//! con el tipo en stdout: runner::THROW_EXIT). Si ademas
//! su stdout es vacio, la comparacion cruda solo puede ver que el minificado
//! lanzo, se colgo o imprimio algo; los valores se comparan en la pasada
//! observada (epilogo). Sin epilogo, dos programas mudos "coinciden" siempre:
//! por eso tests/exec.rs tiene el guard de salida vacia.

use std::path::{Path, PathBuf};

use crate::minify::SourceKind;
use crate::runner::{Exec, Status, DZ, THROW_EXIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// El hallazgo clasico: mismo exit, distinto stdout.
    Stdout,
    /// El minificado lanzo (o murio por senal) y el original no.
    ExitStatus,
    /// El minificado se colgo y el original no. Posible loop introducido.
    Timeout,
    /// El minificado lanza y el original no: el minifier introdujo el error
    /// (p. ej. un SyntaxError, como swc#12397 o el 8953). La mas grave.
    ThrowNew,
    /// El original lanza y el minificado no: suele ser codigo muerto que lanzaba
    /// y se borro. Prioridad mas baja.
    ThrowLost,
    /// Los dos lanzan, tipos distintos.
    ThrowType,
}

pub enum Verdict {
    Same,
    Divergence(Kind),
}

pub fn compare(a: &Exec, b: &Exec) -> Verdict {
    match b.status {
        Status::Timeout => return Verdict::Divergence(Kind::Timeout),
        Status::Exited(0) => {}
        // M10: 70 solo es "lanzo" si el preload lo dijo por su canal.
        Status::Exited(THROW_EXIT) if b.threw() => {}
        Status::Exited(_) | Status::Signal => return Verdict::Divergence(Kind::ExitStatus),
        // Error del fuzzer, no del programa: no es divergencia. main lo cuenta.
        Status::Failed(_) => return Verdict::Same,
    }
    let (na, nb) = (normalize_names(&a.stdout), normalize_names(&b.stdout));
    if na == nb {
        Verdict::Same
    } else {
        let kind = match (throw_line(&na), throw_line(&nb)) {
            (None, Some(_)) => Kind::ThrowNew,
            (Some(_), None) => Kind::ThrowLost,
            (Some(x), Some(y)) if x != y => Kind::ThrowType,
            _ => Kind::Stdout,
        };
        Verdict::Divergence(kind)
    }
}

fn throw_line(s: &str) -> Option<&str> {
    s.lines()
        .rev()
        .find(|l| l.strip_prefix(DZ).is_some_and(|r| r.starts_with("throw ")))
}

/// Borra los nombres de funciones y clases que imprime `util.inspect`
/// (`[Function: f]`, `[Function (anonymous)]`, `[class A extends B]`, ...).
///
/// `keep_fnames` protege nombres explicitos, pero no los inferidos: en
/// `var o = { f: function () {} }` la funcion se llama `f` por la clave, y si
/// `hoist_props` la mueve a una variable mangleada pasa a llamarse `o`. Eso no
/// es un bug (casos 2508_5/6, unsafe_evaluate_object_2). Una funcion sigue sin
/// ser igual a un numero: `[Function]` contra `6` diverge igual.
pub fn normalize_names(s: &str) -> String {
    const OPENERS: [&str; 5] = [
        "[Function",
        "[AsyncFunction",
        "[GeneratorFunction",
        "[AsyncGeneratorFunction",
        "[class",
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some((i, opener)) = OPENERS
        .iter()
        .filter_map(|o| rest.find(o).map(|i| (i, *o)))
        .min_by_key(|(i, _)| *i)
    {
        let after = &rest[i + opener.len()..];
        // Solo si lo que sigue es ':' ' ' o ']' (evita tocar "[Functional]").
        let Some(next) = after.chars().next() else {
            break;
        };
        // M5: el cierre tiene que estar en la misma linea y sin otro '[' en el
        // medio. Si no, se borraba texto real entre lineas (normalize_oculta_pid).
        let close = after
            .find([']', '\n', '['])
            .filter(|&j| after[j..].starts_with(']'));
        let Some(close) = close.filter(|_| matches!(next, ':' | ' ' | ']')) else {
            out.push_str(&rest[..i + opener.len()]);
            rest = after;
            continue;
        };
        out.push_str(&rest[..i]);
        out.push_str(if opener == "[class" { "[class]" } else { "[Function]" });
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// M4: `difuzz-<pid>-w<n>...` (rutas del script, el preload y el cwd por worker,
/// runner.rs) cambian por proceso y por worker. En el sintoma van como
/// `difuzz-PID-wN`: si no, un stack con la ruta hace que una conocida vuelva a
/// `div/` en cada corrida (issue_2208_8). Propuesta de A2, Apendice B.
pub fn normalize_paths(s: &str) -> String {
    let (mut out, mut rest) = (String::with_capacity(s.len()), s);
    while let Some(i) = rest.find("difuzz-") {
        let after = &rest[i + 7..];
        let p = after.bytes().take_while(u8::is_ascii_digit).count();
        if p == 0 || !after[p..].starts_with('-') {
            out.push_str(&rest[..i + 7]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..i]);
        out.push_str("difuzz-PID-");
        let mut tail = &after[p + 1..];
        if let Some(w) = tail.strip_prefix('w') {
            let n = w.bytes().take_while(u8::is_ascii_digit).count();
            if n > 0 {
                out.push_str("wN");
                tail = &w[n..];
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// M4: el hash de sintoma de una divergencia, uno solo para known-sintomas.tsv,
/// el reporter y el indice. Sin nombres de funciones (normalize_names), sin
/// rutas por proceso (normalize_paths) y sin el prefijo de `temp_dir()` (puede
/// cambiar entre corridas con TMPDIR).
pub fn symptom(kind: Kind, a: &Exec, b: &Exec) -> u64 {
    let tmp = std::env::temp_dir().to_string_lossy().trim_end_matches('/').to_string();
    let n = |s: &str| {
        let s = normalize_names(&normalize_paths(s));
        if tmp.is_empty() {
            s
        } else {
            s.replace(&format!("{tmp}/difuzz-PID-"), "TMP/difuzz-PID-")
        }
    };
    crate::known::fnv1a(&[format!("{kind:?}").as_str(), n(&a.stdout).as_str(), n(&b.stdout).as_str()])
}

/// Un directorio por hallazgo con todo lo necesario para reproducirlo a mano.
pub struct Reporter {
    root: PathBuf,
    config_name: String,
    /// Indice compartido entre celdas (`--indice DIR`), si se pidio.
    index: Option<crate::indice::Index>,
    /// Extension de los programas guardados: `cjs` (carril A) o `mjs` (carril B).
    ext: &'static str,
    /// M12: `--invocar` en el envoltorio de modulos que se guarda para reproducir.
    invocar: bool,
    /// M21: version de node y del minifier, para el meta.
    versions: String,
    /// M26: `<out>/descartes.tsv` (caso, causa, elapsed_ms), truncado al crear
    /// el reporter, como div/. `None` si no se pudo crear.
    descartes: std::sync::Mutex<Option<std::fs::File>>,
}

impl Reporter {
    /// Guarda el preload una vez en la raiz: cada hallazgo se reproduce con
    /// `TZ=UTC node --require <out>/preload.cjs original.cjs`.
    ///
    /// Borra `div/`, `known/`, `panic/` y `lint/` de una corrida anterior: si no, los
    /// hallazgos viejos se mezclan con los nuevos y el `ls` miente (paso en la
    /// corrida del 21-sep, con 8 directorios de sobra).
    pub fn new(root: &Path, config_name: String, preload: &str) -> Reporter {
        for sub in ["div", "known", "panic", "lint", "asserts"] {
            let _ = std::fs::remove_dir_all(root.join(sub));
        }
        write_all(root, &[("preload.cjs", preload)]);
        let descartes = std::fs::File::create(root.join("descartes.tsv"))
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(b"# difuzz descartes: caso\tcausa\telapsed_ms\n").map(|_| f)
            })
            .map_err(|e| {
                eprintln!("reporter: no pude crear {}/descartes.tsv: {e}", root.display());
                write_failed();
            })
            .ok();
        Reporter {
            descartes: std::sync::Mutex::new(descartes),
            root: root.to_path_buf(),
            config_name,
            ext: SourceKind::Script.ext(),
            index: None,
            invocar: false,
            versions: String::new(),
        }
    }

    /// M21: `node: v24...\nbackend: oxc_minifier 0.153.0\n` en cada meta.
    pub fn with_versions(mut self, node: &str, backend: &str) -> Reporter {
        self.versions = format!("node: {node}\nbackend: {backend}\ndebug_assertions: {}\n", debug_assertions());
        self
    }

    pub fn with_invocar(mut self, invocar: bool) -> Reporter {
        self.invocar = invocar;
        self
    }

    /// M26: un caso que no se comparo y por que (las causas de `descartados:`
    /// en la linea de stats, mas parse y codegen).
    pub fn discard(&self, case: &Path, cause: &str, elapsed: std::time::Duration) {
        use std::io::Write;
        let mut f = self.descartes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = f.as_mut() {
            let line = format!("{}\t{cause}\t{}\n", case.display(), elapsed.as_millis());
            if let Err(e) = f.write_all(line.as_bytes()) {
                eprintln!("reporter: no pude escribir descartes.tsv: {e}");
            }
        }
    }

    /// M23: cierra el indice (si hay): `complete` lo publica, si no lo descarta.
    pub fn finish(&self, complete: bool) {
        if let Some(ix) = &self.index {
            if complete {
                if let Err(e) = ix.commit() {
                    eprintln!("indice: no pude cerrar: {e}");
                }
            } else {
                ix.abandon();
            }
        }
    }

    pub fn with_index(mut self, index: crate::indice::Index) -> Reporter {
        self.index = Some(index);
        self
    }

    /// Una linea en el indice (si hay): el caso es el nombre de `dir`.
    fn index(&self, dir: &Path, dest: &str, kind: &str, hash: u64) {
        if let Some(ix) = &self.index {
            let case = dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            ix.record(&case, dest, kind, hash);
        }
    }

    /// Carril B: los programas se guardan como `.mjs` (un `.cjs` con `import`
    /// no se puede reproducir).
    pub fn with_kind(mut self, kind: SourceKind) -> Reporter {
        self.ext = kind.ext();
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn divergence(
        &self,
        case: &Path,
        original: &str,
        minified: &str,
        a: &Exec,
        b: &Exec,
        kind: Kind,
        observed: bool,
        known: Option<&str>,
        note: Option<&str>,
        flags_a: &[&str],
        flags_b: &[&str],
        a_origin: &str,
    ) {
        let list = |f: &[&str]| if f.is_empty() { "-".to_string() } else { f.join(",") };
        let dest = if known.is_some() { "known" } else { "div" };
        let dir = self.case_dir(dest, case);
        // Mismo sintoma que known-sintomas.tsv (main::compare_and_report).
        let symptom = symptom(kind, a, b);
        self.index(&dir, dest, &format!("{kind:?}"), symptom);
        let (orig_file, min_file) = (format!("original.{}", self.ext), format!("minified.{}", self.ext));
        // Carril B observado: lo que se comparo es la salida del envoltorio que
        // serializa el namespace. Se guarda uno por lado, para reproducir igual.
        let module_observed = observed && self.ext == SourceKind::Module.ext();
        let (repro_a, repro_b) = if module_observed {
            ("observar-original.mjs", "observar-minified.mjs")
        } else {
            (orig_file.as_str(), min_file.as_str())
        };
        // M21: el comando exacto del runner (--no-warnings, entorno minimo), uno por
        // lado, a correr desde el directorio del hallazgo.
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let cmd = |f: &str| {
            format!(
                "cd {} && env -i PATH=\"$PATH\" TZ=UTC HOME=/tmp node --no-warnings --require {}/preload.cjs {f}",
                abs(&dir).display(),
                abs(&self.root).display()
            )
        };
        let meta = format!(
            "case: {}\nkind: {kind:?}\nconfig: {}\nobserved: {observed}\nknown: {}\nnota: {}\nflags_a: {}\nflags_b: {}\n\noriginal: {:?} en {:?}\nminificado: {:?} en {:?}\na_cache: {a_origin}\n{}\nrepro_original: {}\nrepro_minificado: {}\n",
            case.display(),
            self.config_name,
            known.unwrap_or("no"),
            note.unwrap_or("-"),
            list(flags_a),
            list(flags_b),
            a.status,
            a.elapsed,
            b.status,
            b.elapsed,
            self.versions,
            cmd(repro_a),
            cmd(repro_b),
        );
        // R1-H05: el minificado salio del oraculo sin debug assertions (el binario
        // con asserts hizo panic). Al final del meta: el resto sigue igual. Si el
        // oraculo se uso en otra pasada del caso (re-chequeo), se dice eso.
        let meta = match crate::minify::nota_asserts() {
            Some(n) if n.minificado == minified => format!("{meta}{}", nota_solo_asserts(&n.panic)),
            Some(n) => format!(
                "{meta}\notra_pasada_con_oraculo: si (este minificado no; un re-chequeo u otra pasada del caso hizo panic con debug assertions y uso el oraculo)\npanic_con_debug_assertions:\n{}\n",
                n.panic.trim_end()
            ),
            None => meta,
        };
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                (orig_file.as_str(), original),
                (min_file.as_str(), minified),
                ("a.stdout", a.stdout.as_str()),
                ("a.stderr", a.stderr.as_str()),
                ("b.stdout", b.stdout.as_str()),
                ("b.stderr", b.stderr.as_str()),
            ],
        );
        // M21: mensaje y stack del throw (no se comparan: traen nombres mangleados).
        for (name, e) in [("a.exc", a), ("b.exc", b)] {
            if let Some(text) = e.exc() {
                write_all(&dir, &[(name, text.as_str())]);
            }
        }
        if module_observed {
            write_all(
                &dir,
                &[
                    ("observar-original.mjs", crate::runner::module_wrapper(&orig_file, self.invocar).as_str()),
                    ("observar-minified.mjs", crate::runner::module_wrapper(&min_file, self.invocar).as_str()),
                ],
            );
        }
    }

    /// Sospechoso, no veredicto: el minificado escribe sobre literales frescos
    /// mas veces que el original (checks::dead_writes). Se triagea a mano en
    /// lint-triage.tsv (triage.rs); `triage` es la nota de ahi.
    pub fn lint(&self, case: &Path, original: &str, minified: &str, counts: (usize, usize), triage: &str) {
        let dir = self.case_dir("lint", case);
        self.index(&dir, "lint", "EscriturasSobreLiterales", crate::known::fnv1a(&[minified]));
        let meta = format!(
            "case: {}\nkind: EscriturasSobreLiterales\nconfig: {}\nen original: {}\nen minificado: {}\ntriage: {triage}\n\nsospechoso, no veredicto: buscar en minified.cjs una asignacion cuyo destino es [...] o {{...}}\n",
            case.display(),
            self.config_name,
            counts.0,
            counts.1,
        );
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                (format!("original.{}", self.ext).as_str(), original),
                (format!("minified.{}", self.ext).as_str(), minified),
            ],
        );
    }

    pub fn panic(&self, case: &Path, src: &str, message: &str) {
        let dir = self.case_dir("panic", case);
        self.index(&dir, "panic", "Panic", crate::known::fnv1a(&[message]));
        // M22: versiones y debug_assertions (un debug_assert de oxc solo salta
        // porque el binario los lleva prendidos: Cargo.toml [profile.release]).
        let meta = format!(
            "case: {}\nkind: Panic\nconfig: {}\n{}\n{message}\n",
            case.display(),
            self.config_name,
            if self.versions.is_empty() { format!("debug_assertions: {}\n", debug_assertions()) } else { self.versions.clone() },
        );
        write_all(&dir, &[("meta.txt", meta.as_str()), ("input.js", src)]);
    }

    /// R1-H05: el binario con debug assertions hizo panic, el oraculo sin ellos
    /// minifico y el caso NO divergio (o se descarto). No es un hallazgo de
    /// panic/ ni de div/: queda en `asserts/` como senal para los asserts.
    /// `original` y `minified` son los de la pasada cruda del caso; `minified.*`
    /// guarda lo que devolvio el oraculo (si fue en otra pasada, el meta lo dice
    /// y `original.*` no se guarda: seria de otro programa).
    pub fn solo_asserts(&self, case: &Path, src: &str, original: &str, minified: &str, nota: &crate::minify::NotaAsserts) {
        let dir = self.case_dir("asserts", case);
        let misma = nota.minificado == minified;
        let meta = format!(
            "case: {}\nkind: SoloDebugAssertions\nconfig: {}\n{}sin divergencia con el minificado del oraculo (o el caso se descarto: ver descartes.tsv)\n{}{}",
            case.display(),
            self.config_name,
            if self.versions.is_empty() { format!("debug_assertions: {}\n", debug_assertions()) } else { self.versions.clone() },
            if misma { "" } else { "pasada: no la cruda (observador o re-chequeo); minified.* es de ese programa\n" },
            nota_solo_asserts(&nota.panic),
        );
        let (orig_file, min_file) = (format!("original.{}", self.ext), format!("minified.{}", self.ext));
        write_all(&dir, &[("meta.txt", meta.as_str()), ("input.js", src), (min_file.as_str(), nota.minificado.as_str())]);
        if misma {
            write_all(&dir, &[(orig_file.as_str(), original)]);
        }
    }

    /// Agrega archivos a los directorios que ya existen para el caso (div, known,
    /// panic, lint). Lo usan las mutaciones: la variante va tal cual junto al hallazgo.
    pub fn attach(&self, case: &Path, files: &[(&str, &str)]) {
        for prefix in ["div", "known", "panic", "lint"] {
            let dir = self.case_dir(prefix, case);
            if dir.is_dir() {
                write_all(&dir, files);
            }
        }
    }

    /// `<root>/<prefijo>/<ruta del caso con '/' → '__'>`. Si el corpus es el
    /// directorio del caso mismo, la ruta relativa no tiene padre: `_root`.
    fn case_dir(&self, prefix: &str, case: &Path) -> PathBuf {
        let slug: Vec<String> = case
            .parent()
            .unwrap_or(case)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|c| c != "." && c != "/")
            .collect();
        let name = if slug.is_empty() { "_root".to_string() } else { slug.join("__") };
        self.root.join(prefix).join(name)
    }
}

/// R1-H05: el bloque del meta para un caso minificado por el oraculo.
fn nota_solo_asserts(panic: &str) -> String {
    format!(
        "\nsolo_debug_assertions: si (con debug assertions el minifier hizo panic; el minificado es del oraculo sin debug assertions, como npm)\npanic_con_debug_assertions:\n{}\n",
        panic.trim_end()
    )
}

fn debug_assertions() -> &'static str {
    if cfg!(debug_assertions) {
        "on"
    } else {
        "off"
    }
}

/// M27 (a): escrituras del reporter que fallaron en este proceso. main sale con
/// error si hubo alguna (antes: exit 0 y sin hallazgos con un --out sin permisos).
static WRITE_ERRORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn write_errors() -> usize {
    WRITE_ERRORS.load(std::sync::atomic::Ordering::Relaxed)
}

fn write_failed() {
    WRITE_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn write_all(dir: &Path, files: &[(&str, &str)]) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("reporter: no pude crear {}: {e}", dir.display());
        write_failed();
        return;
    }
    for (name, content) in files {
        if let Err(e) = std::fs::write(dir.join(name), content) {
            eprintln!("reporter: no pude escribir {}/{name}: {e}", dir.display());
            write_failed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_names as n;

    #[test]
    fn borra_nombres_de_funciones_y_clases() {
        assert_eq!(n("[Function: f]"), "[Function]");
        assert_eq!(n("1 2 4 [Function (anonymous)]"), "1 2 4 [Function]");
        assert_eq!(
            n("[class B extends A] [AsyncFunction: x] [Function: g] { a: 1 }"),
            "[class] [Function] [Function] { a: 1 }"
        );
    }

    #[test]
    fn no_toca_lo_que_no_es_un_nombre() {
        assert_eq!(n("[Functional] [Function"), "[Functional] [Function");
        assert_eq!(n("sin nada"), "sin nada");
    }

    /// M5: el nombre no cruza lineas ni corchetes anidados.
    #[test]
    fn el_nombre_no_cruza_lineas() {
        assert_ne!(n("[Function \n1\n]"), n("[Function \n2\n]"));
        assert_ne!(n("[class [1] 2]"), n("[class [3] 4]"));
        assert_eq!(n("[Function: f]\n[class A]"), "[Function]\n[class]");
    }

    #[test]
    fn throw_line_toma_la_ultima_linea_de_difuzz() {
        use super::throw_line;
        assert_eq!(throw_line("1\n!dz throw TypeError\n"), Some("!dz throw TypeError"));
        assert_eq!(throw_line("!throw TypeError\n"), None);
        assert_eq!(throw_line("1\n2\n"), None);
    }

    /// M4: el pid y el worker no cambian el sintoma.
    #[test]
    fn sintoma_sin_rutas_por_proceso() {
        use super::{normalize_paths, symptom, Kind};
        use crate::runner::{Exec, Status};
        assert_eq!(normalize_paths("at /tmp/difuzz-123-w4.cjs:1:2"), "at /tmp/difuzz-PID-wN.cjs:1:2");
        assert_eq!(normalize_paths("difuzz-77-preload.cjs"), "difuzz-PID-preload.cjs");
        assert_eq!(normalize_paths("difuzz-x difuzz-12 difuzz-"), "difuzz-x difuzz-12 difuzz-");
        let ex = |s: &str| Exec {
            status: Status::Exited(0),
            stdout: s.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let tmp = std::env::temp_dir();
        let (a1, a2) = (
            ex(&format!("at {}/difuzz-123-w4.cjs:1:1\n", tmp.display())),
            ex(&format!("at {}/difuzz-999-w1.cjs:1:1\n", tmp.display())),
        );
        let b = ex("x\n");
        assert_eq!(symptom(Kind::Stdout, &a1, &b), symptom(Kind::Stdout, &a2, &b));
        assert_ne!(symptom(Kind::Stdout, &a1, &b), symptom(Kind::Stdout, &ex("otra\n"), &b));
    }

    /// M21: el meta trae los dos comandos de repro, el origen de A y las
    /// versiones; el mensaje del throw va a b.exc.
    #[test]
    fn meta_reproducible() {
        use super::{Kind, Reporter};
        use crate::runner::{Exec, Status};
        let d = std::env::temp_dir().join(format!("difuzz-test-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let r = Reporter::new(&d, "celda".into(), "//preload").with_versions("v0", "x 1");
        let ex = |out: &str, ctl: &str| Exec {
            status: Status::Exited(0),
            stdout: out.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: ctl.into(),
        };
        let (a, b) = (ex("1\n", ""), ex("!dz throw TypeError\n", "throw TypeError\nexc TypeError: x\\n    at y\n"));
        r.divergence(std::path::Path::new("c/input.js"), "o", "m", &a, &b, Kind::ThrowNew, false, None, None, &[], &[], "miss");
        let meta = std::fs::read_to_string(d.join("div/c/meta.txt")).unwrap();
        for want in ["a_cache: miss\n", "node: v0\n", "backend: x 1\n", "debug_assertions: ", "repro_original: cd /", "--no-warnings --require /", "original.cjs\n", "repro_minificado: cd /"] {
            assert!(meta.contains(want), "falta {want:?} en\n{meta}");
        }
        assert_eq!(std::fs::read_to_string(d.join("div/c/b.exc")).unwrap(), "TypeError: x\n    at y");
        assert!(!d.join("div/c/a.exc").exists());
        // M26: descartes.tsv, truncado al crear el reporter.
        r.discard(std::path::Path::new("c/input.js"), "timeout", std::time::Duration::from_millis(3001));
        assert!(std::fs::read_to_string(d.join("descartes.tsv")).unwrap().ends_with("\nc/input.js\ttimeout\t3001\n"));
        drop(r);
        let _ = Reporter::new(&d, "celda".into(), "//preload");
        assert_eq!(std::fs::read_to_string(d.join("descartes.tsv")).unwrap().lines().count(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn una_funcion_sigue_sin_ser_un_numero() {
        // inverted_var: [Function: c] contra 6 tiene que seguir divergiendo.
        assert_ne!(n("1 2 [Function: c] 7"), n("1 2 6 7"));
    }
}
