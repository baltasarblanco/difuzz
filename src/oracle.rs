//! CMP: que cuenta como divergencia (delta v1.2 §1 §3.4) y como se guarda.
//!
//! Precondicion que garantiza main: el original termino solo (exit 0, o lanzando
//! con el tipo en stdout: runner::THROW_EXIT). Si ademas
//! su stdout es vacio, la comparacion cruda solo puede ver que el minificado
//! lanzo, se colgo o imprimio algo; los valores se comparan en la pasada
//! observada (epilogo). Sin epilogo, dos programas mudos "coinciden" siempre:
//! por eso tests/exec.rs tiene el guard de salida vacia.

use std::path::{Path, PathBuf};

use crate::runner::{Exec, Status, DZ, THROW_EXIT};

#[derive(Debug, Clone, Copy)]
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
        Status::Exited(0) | Status::Exited(THROW_EXIT) => {}
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
        let Some(close) = after.find(']').filter(|_| matches!(next, ':' | ' ' | ']')) else {
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

/// Un directorio por hallazgo con todo lo necesario para reproducirlo a mano.
pub struct Reporter {
    root: PathBuf,
    config_name: String,
}

impl Reporter {
    /// Guarda el preload una vez en la raiz: cada hallazgo se reproduce con
    /// `TZ=UTC node --require <out>/preload.cjs original.cjs`.
    ///
    /// Borra `div/`, `known/`, `panic/` y `lint/` de una corrida anterior: si no, los
    /// hallazgos viejos se mezclan con los nuevos y el `ls` miente (paso en la
    /// corrida del 21-sep, con 8 directorios de sobra).
    pub fn new(root: &Path, config_name: String, preload: &str) -> Reporter {
        for sub in ["div", "known", "panic", "lint"] {
            let _ = std::fs::remove_dir_all(root.join(sub));
        }
        write_all(root, &[("preload.cjs", preload)]);
        Reporter {
            root: root.to_path_buf(),
            config_name,
        }
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
    ) {
        let list = |f: &[&str]| if f.is_empty() { "-".to_string() } else { f.join(",") };
        let dir = self.case_dir(if known.is_some() { "known" } else { "div" }, case);
        let meta = format!(
            "case: {}\nkind: {kind:?}\nconfig: {}\nobserved: {observed}\nknown: {}\nnota: {}\nflags_a: {}\nflags_b: {}\n\noriginal: {:?} en {:?}\nminificado: {:?} en {:?}\n\nrepro: TZ=UTC node --require {}/preload.cjs original.cjs\n",
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
            self.root.display(),
        );
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                ("original.cjs", original),
                ("minified.cjs", minified),
                ("a.stdout", a.stdout.as_str()),
                ("a.stderr", a.stderr.as_str()),
                ("b.stdout", b.stdout.as_str()),
                ("b.stderr", b.stderr.as_str()),
            ],
        );
    }

    /// Sospechoso, no veredicto: el minificado escribe sobre literales frescos
    /// mas veces que el original (checks::dead_writes). Se triagea a mano.
    pub fn lint(&self, case: &Path, original: &str, minified: &str, counts: (usize, usize)) {
        let dir = self.case_dir("lint", case);
        let meta = format!(
            "case: {}\nkind: EscriturasSobreLiterales\nconfig: {}\nen original: {}\nen minificado: {}\n\nsospechoso, no veredicto: buscar en minified.cjs una asignacion cuyo destino es [...] o {{...}}\n",
            case.display(),
            self.config_name,
            counts.0,
            counts.1,
        );
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                ("original.cjs", original),
                ("minified.cjs", minified),
            ],
        );
    }

    pub fn panic(&self, case: &Path, src: &str, message: &str) {
        let dir = self.case_dir("panic", case);
        let meta = format!(
            "case: {}\nkind: Panic\nconfig: {}\n\n{message}\n",
            case.display(),
            self.config_name
        );
        write_all(&dir, &[("meta.txt", meta.as_str()), ("input.js", src)]);
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

fn write_all(dir: &Path, files: &[(&str, &str)]) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("reporter: no pude crear {}: {e}", dir.display());
        return;
    }
    for (name, content) in files {
        if let Err(e) = std::fs::write(dir.join(name), content) {
            eprintln!("reporter: no pude escribir {}/{name}: {e}", dir.display());
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

    #[test]
    fn throw_line_toma_la_ultima_linea_de_difuzz() {
        use super::throw_line;
        assert_eq!(throw_line("1\n!dz throw TypeError\n"), Some("!dz throw TypeError"));
        assert_eq!(throw_line("!throw TypeError\n"), None);
        assert_eq!(throw_line("1\n2\n"), None);
    }

    #[test]
    fn una_funcion_sigue_sin_ser_un_numero() {
        // inverted_var: [Function: c] contra 6 tiene que seguir divergiendo.
        assert_ne!(n("1 2 [Function: c] 7"), n("1 2 6 7"));
    }
}
