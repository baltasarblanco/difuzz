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
    // n-ruido: sin las posiciones de los marcos de stack (normalize_stack_positions).
    // Solo aca: los re-chequeos que miran lineas (recheck::localizada*) ven el marco
    // entero, con su posicion, para reconocerlo como marco.
    // 7.5: solo de los archivos de marcos que V8 formateo en A o en B (y los de difuzz).
    let archivos = archivos_de_marco(a, b);
    let n = |s: &str| normalize_stack_positions(&nombres_de_celda(s), &archivos);
    let (na, nb) = (n(&a.stdout), n(&b.stdout));
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

/// 10.1 (`--crudo`): `x` e `y`, dos programas que tienen que ser el mismo (el texto crudo y el
/// original que imprime el backend, A), dicen lo mismo: el status y el stdout iguales, con las
/// posiciones de los marcos de stack borradas como en `compare` (al reimprimir cambian lineas y
/// columnas: issue_2208_8 imprime un stack en stdout). A diferencia de `main::same_exec` y de
/// `compare`, los nombres que imprime `util.inspect` quedan crudos: parsear e imprimir no
/// tiene por que tocarlos (el bug de los parentesis de `(h) = function(){}` cambia uno).
pub fn mismo_programa(x: &Exec, y: &Exec) -> bool {
    let archivos = archivos_de_marco(x, y);
    let n = |s: &str| normalize_stack_positions(s, &archivos);
    x.status == y.status && n(&x.stdout) == n(&y.stdout)
}

/// 8.3: los stdout de A y B normalizados igual que en `compare` (para
/// `recheck::mensaje_con_nombre_localizado`).
pub fn normalizadas(a: &Exec, b: &Exec) -> (String, String) {
    let archivos = archivos_de_marco(a, b);
    let n = |s: &str| normalize_stack_positions(&nombres_de_celda(s), &archivos);
    (n(&a.stdout), n(&b.stdout))
}

thread_local! {
    /// 9.1: ver `con_nombres_crudos`. Por hilo: cada worker procesa un caso a la vez y todas
    /// las comparaciones del caso (la de main y las de los re-chequeos) corren en su hilo.
    static NOMBRES_CRUDOS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 9.1 (H3 de la revision del bloque 8): mientras viva lo que devuelve, las comparaciones de
/// este hilo (`compare`, `normalizadas`, `symptom`, `nombres_de_celda`) conservan, si `crudos`,
/// los nombres que imprime `util.inspect` en vez de borrarlos con `normalize_names`. Lo fija
/// `main::process_case` para todo el caso con `Minifier::preserva_nombres` de la celda: con
/// keep_names (carril_a) el minifier promete el `.name` de funciones y clases, y
/// `[Function: handler]` contra `[Function: e]` es un bug suyo, no ruido. Sin keep_names
/// (core_defaults) se borran como siempre. Un hilo que no lo fija (tests) los borra.
pub fn con_nombres_crudos(crudos: bool) -> NombresDeCelda {
    NombresDeCelda(NOMBRES_CRUDOS.with(|c| c.replace(crudos)))
}

/// Restaura el modo anterior del hilo al salir (tambien si el caso entra en panico).
pub struct NombresDeCelda(bool);

impl Drop for NombresDeCelda {
    fn drop(&mut self) {
        NOMBRES_CRUDOS.with(|c| c.set(self.0));
    }
}

/// 9.1: `compare` da Same si se borran los nombres de `util.inspect` (como en una celda sin
/// keep_names). Con los nombres crudos y una divergencia, es que A y B solo difieren en esos
/// nombres: la traza lo dice (`main::compare_and_report`).
pub fn solo_nombres_de_inspect(a: &Exec, b: &Exec) -> bool {
    let _borrados = con_nombres_crudos(false);
    matches!(compare(a, b), Verdict::Same)
}

/// `s` con los nombres de `util.inspect` borrados (`normalize_names`), salvo que el caso en
/// curso los compare crudos (`con_nombres_crudos`). Para comparar A con B, o con algo que
/// hace las veces de B; A contra A (`main::same_exec`) sigue con `normalize_names`.
pub fn nombres_de_celda(s: &str) -> String {
    if NOMBRES_CRUDOS.with(std::cell::Cell::get) {
        s.to_string()
    } else {
        normalize_names(s)
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
///
/// 9.1: las comparaciones de A con B pasan por `nombres_de_celda`, que con keep_names en la
/// celda no llama a esta funcion.
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

/// n-ruido (t-real: ~12 casos en known/ solo por columnas): en un marco de stack
/// (`at f (archivo:L:C)`, `at archivo:L:C`) cambia cada `archivo:L:C` por `<pos>`:
/// archivo, linea y columna cambian legitimamente al minificar. Conserva el resto del
/// marco (nombre de la funcion, `new`, `async`, `[as x]`, `<anonymous>` sin posicion,
/// `native`) y la cantidad y el orden de los marcos: un bug que se ve en el stack fuera
/// de las posiciones sigue divergiendo (los nombres tienen su propio re-chequeo).
///
/// Un marco empieza en `at ` precedido por uno o mas espacios y, antes de ellos, el
/// comienzo de la linea o un `\n` escapado (un stack guardado en un string sale en
/// JSON, en una sola linea). Termina en el fin de la linea, en otro `\n` escapado o en
/// una comilla. Solo se toca un `:L:C` seguido del fin del marco, `)` o `,`.
///
/// 7.5 (revision n2, MENOR 5): y solo si el archivo es de un marco real (`es_archivo_de_marco`):
/// uno de `reales` (los que el preload anoto al formatear un stack en la corrida, lineas
/// `marco` del canal: el programa, `node:...`, `<anonymous>`, un `//# sourceURL`) o uno de
/// difuzz (`difuzz-<pid>-...`, el programa aunque el stack no pase por el preload). Una
/// linea impresa con forma de marco y otro archivo (`    at main (input.js:3:4)`) conserva
/// su posicion. Limite: una linea impresa a proposito con el archivo de un marco real
/// (`__filename`, o el mismo nombre que un `sourceURL` del programa) la pierde igual.
pub fn normalize_stack_positions(s: &str, reales: &[&str]) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let (mut copied, mut i) = (0, 0);
        while let Some(k) = line[i..].find("at ") {
            let p = i + k;
            let antes = &line[..p];
            let sin = antes.trim_end_matches(' ');
            if sin.len() == antes.len() || !(sin.is_empty() || sin.ends_with("\\n")) {
                i = p + 3;
                continue;
            }
            let cuerpo = p + 3;
            let fin = [line[cuerpo..].find("\\n"), line[cuerpo..].find('"'), line[cuerpo..].find('\n')]
                .into_iter()
                .flatten()
                .min()
                .map_or(line.len(), |j| cuerpo + j);
            out.push_str(&line[copied..cuerpo]);
            out.push_str(&sin_posiciones(&line[cuerpo..fin], reales));
            (copied, i) = (fin, fin);
        }
        out.push_str(&line[copied..]);
    }
    out
}

/// El cuerpo de un marco con cada `archivo:L:C` cambiado por `<pos>` (el archivo va
/// desde el ultimo espacio o `(` anterior, sin pasar del fin de la posicion anterior). 7.5:
/// solo si `es_archivo_de_marco(archivo, reales)`. El preload anota los archivos con el
/// mismo recorrido (`anotarArchivosDeMarco`, preload.js).
fn sin_posiciones(f: &str, reales: &[&str]) -> String {
    let b = f.as_bytes();
    let digitos = |k: usize| b[k..].iter().take_while(|c| c.is_ascii_digit()).count();
    let (mut out, mut copied, mut desde, mut i) = (String::with_capacity(f.len()), 0, 0, 0);
    while i < b.len() {
        if b[i] == b':' {
            let n1 = digitos(i + 1);
            if n1 > 0 && b.get(i + 1 + n1) == Some(&b':') {
                let k = i + 2 + n1;
                let j = k + digitos(k);
                if j > k && (j == b.len() || matches!(b[j], b')' | b',')) {
                    let start = f[desde..i].rfind([' ', '(']).map_or(desde, |s| desde + s + 1);
                    if es_archivo_de_marco(&f[start..i], reales) {
                        out.push_str(&f[copied..start]);
                        out.push_str("<pos>");
                        copied = j;
                    }
                    (desde, i) = (j, j);
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&f[copied..]);
    out
}

/// 7.5: prefijo de las lineas del canal con los archivos de los marcos que formateo el
/// preload (`marco <archivo>`, una por archivo).
pub const MARCO: &str = "marco ";

/// 7.5: los archivos de marco anotados en el canal de A y de B (sin repetir).
fn archivos_de_marco<'a>(a: &'a Exec, b: &'a Exec) -> Vec<&'a str> {
    let mut v: Vec<&str> = a.ctl.lines().chain(b.ctl.lines()).filter_map(|l| l.strip_prefix(MARCO)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// 7.5: `archivo` es el de un marco real: uno que el preload vio en un stack formateado
/// (`reales`) o un archivo de difuzz (`difuzz-<pid>-...`, como en `normalize_paths`: el
/// programa, su envoltorio, el preload), que cubre los stacks que no pasan por el preload
/// (`Error.prepareStackTrace = undefined`, como hace React, o uno propio del programa).
fn es_archivo_de_marco(archivo: &str, reales: &[&str]) -> bool {
    if archivo.is_empty() {
        return false;
    }
    if reales.contains(&archivo) {
        return true;
    }
    let base = archivo.rsplit('/').next().unwrap_or(archivo);
    base.strip_prefix("difuzz-").is_some_and(|r| {
        let p = r.bytes().take_while(u8::is_ascii_digit).count();
        p > 0 && r[p..].starts_with('-')
    })
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
/// el reporter y el indice. Sin nombres de funciones (normalize_names; 9.1: salvo con
/// keep_names en la celda, `nombres_de_celda`), sin
/// rutas por proceso (normalize_paths) y sin el prefijo de `temp_dir()` (puede
/// cambiar entre corridas con TMPDIR).
pub fn symptom(kind: Kind, a: &Exec, b: &Exec) -> u64 {
    let tmp = std::env::temp_dir().to_string_lossy().trim_end_matches('/').to_string();
    let n = |s: &str| {
        let s = nombres_de_celda(&normalize_paths(s));
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
    /// Guarda el preload una vez en la raiz (`<out>/<celda>/`): cada hallazgo se
    /// reproduce con las lineas `repro_original`/`repro_minificado` de su `meta.txt`
    /// (`cd <caso> && env -i PATH="$PATH" TZ=UTC HOME=/tmp node --no-warnings
    /// --require <raiz>/preload.cjs original.cjs`; M21, en `divergence`).
    ///
    /// Borra `div/`, `known/`, `panic/`, `lint/`, `asserts/`, `recursos/` y `codegen/` (10.1) de una
    /// corrida anterior: si no, los
    /// hallazgos viejos se mezclan con los nuevos y el `ls` miente (paso en la
    /// corrida del 21-sep, con 8 directorios de sobra).
    pub fn new(root: &Path, config_name: String, preload: &str) -> Reporter {
        for sub in ["div", "known", "panic", "lint", "asserts", "recursos", "codegen"] {
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
                // 10.4 (M1 de rev-b9-A): como `new` cuando no lo puede crear.
                write_failed();
            }
        }
    }

    /// M23: cierra el indice (si hay): `Completa` lo publica; si no, queda al costado (10.2b:
    /// `.incompleta` si se corto, `.parcial` si termino con escrituras perdidas o casos sin contar).
    pub fn finish(&self, cierre: crate::indice::Cierre) {
        if let Some(ix) = &self.index {
            if let Err(e) = ix.cerrar(cierre) {
                eprintln!("indice: no pude cerrar: {e}");
                // 10.4 (M1 de rev-b9-A): queda el indice anterior y `--cruce` lo muestra sin
                // aviso; antes la corrida cerraba `completa` y con 0. `main` lo cuenta despues.
                write_failed();
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
        traza: &str,
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
        // A6/R1-H12: el analisis de difuzz (no el minifier) fallo en este caso.
        let notas = crate::analisis::notas();
        let meta = if notas.is_empty() { meta } else { format!("{meta}
analisis_de_difuzz_fallo:
{}
", notas.join("\n")) };
        // 7d: la traza de la clasificacion (clasificar.rs), al final: lo de
        // arriba queda igual que antes.
        let meta = if traza.is_empty() { meta } else { format!("{meta}\n{traza}") };
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
            // R2-H08: la extension del carril (en B el archivo es minified.mjs).
            "case: {}\nkind: EscriturasSobreLiterales\nconfig: {}\nen original: {}\nen minificado: {}\ntriage: {triage}\n\nsospechoso, no veredicto: buscar en minified.{} una asignacion cuyo destino es [...] o {{...}}\n",
            case.display(),
            self.config_name,
            counts.0,
            counts.1,
            self.ext,
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

    /// R1-H08 / R2-H04: el hijo minificador no termino a tiempo (`timeout`) o
    /// paso el presupuesto de memoria (`memoria`). No es un panic: va a
    /// `recursos/`, con la causa en el meta (el mensaje trae tiempo o RSS).
    pub fn recurso(&self, case: &Path, src: &str, causa: &str, message: &str) {
        let dir = self.case_dir("recursos", case);
        self.index(&dir, "recursos", "Recursos", crate::known::fnv1a(&[causa]));
        let meta = format!(
            "case: {}\nkind: Recursos\ncausa: {causa}\nconfig: {}\n{}\n{message}\n",
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

    /// 10.1 (`--crudo`): el texto crudo (`crudo`, lo que lee el backend) y el original que el
    /// backend imprime (A, `original`) no dan lo mismo en node, y es determinista (`main::comparar_crudo`).
    /// No es un hallazgo del minificador (A y B salen del mismo programa parseado: cargan el
    /// cambio igual): va a `codegen/`, ortogonal como `lint/` (el caso cuenta tambien donde
    /// termino la comparacion de A con B). `kind` lee al crudo como el original y a A como el
    /// minificado (ThrowNew: A lanza y el crudo no). `pasos`: como se confirmo.
    #[allow(clippy::too_many_arguments)]
    pub fn codegen(&self, case: &Path, crudo: &str, original: &str, r: &Exec, a: &Exec, kind: Kind, pasos: &str) {
        let dir = self.case_dir("codegen", case);
        // 10.10 (A-H6 de la revision A de 10.1): el sintoma con los nombres crudos, como compara
        // `mismo_programa` y como lee el kind `main::chequeo_crudo`; antes, segun la celda (en
        // core_defaults dos hallazgos que solo difieren en un nombre daban el mismo hash).
        let hash = {
            let _crudos = con_nombres_crudos(true);
            symptom(kind, r, a)
        };
        self.index(&dir, "codegen", &format!("{kind:?}"), hash);
        let (crudo_file, orig_file) = (format!("crudo.{}", self.ext), format!("original.{}", self.ext));
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let cmd = |f: &str| {
            format!(
                "cd {} && env -i PATH=\"$PATH\" TZ=UTC HOME=/tmp node --no-warnings --require {}/preload.cjs {f}",
                abs(&dir).display(),
                abs(&self.root).display()
            )
        };
        let meta = format!(
            "case: {}\nkind: {kind:?}\nconfig: {}\nnota: el texto crudo y el original que imprime el backend (A) dan otra salida en node, y se repite: el parser o el codegen del backend (o su cadena antes del minificador) cambiaron la semantica. No es una divergencia del minificador: A y B salen del mismo programa parseado y cargan el cambio igual (10.1, --crudo)\npasos: {pasos}\n\ncrudo: {:?} en {:?}\noriginal: {:?} en {:?}\n{}\nrepro_crudo: {}\nrepro_original: {}\n",
            case.display(),
            self.config_name,
            r.status,
            r.elapsed,
            a.status,
            a.elapsed,
            self.versions,
            cmd(&crudo_file),
            cmd(&orig_file),
        );
        write_all(
            &dir,
            &[
                ("meta.txt", meta.as_str()),
                (crudo_file.as_str(), crudo),
                (orig_file.as_str(), original),
                ("crudo.stdout", r.stdout.as_str()),
                ("crudo.stderr", r.stderr.as_str()),
                ("a.stdout", a.stdout.as_str()),
                ("a.stderr", a.stderr.as_str()),
            ],
        );
        for (name, e) in [("crudo.exc", r), ("a.exc", a)] {
            if let Some(text) = e.exc() {
                write_all(&dir, &[(name, text.as_str())]);
            }
        }
    }

    /// Agrega archivos a los directorios que ya existen para el caso (div, known,
    /// panic, lint). Lo usan las mutaciones: la variante va tal cual junto al hallazgo.
    pub fn attach(&self, case: &Path, files: &[(&str, &str)]) {
        for prefix in ["div", "known", "panic", "lint", "recursos"] {
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

/// R1-H09: cada celda escribe en su propio directorio, `<out>/<celda>/`: dos
/// celdas con el mismo `--out` ya no se borran los hallazgos (ni la tabla de
/// sintomas) entre si, y otra corrida de la misma celda reemplaza solo lo suyo.
/// En la raiz de `--out` quedan lo compartible: la cache de A (`cache-a`, por
/// contenido) y `lint-triage.tsv` (por hash del minificado).
pub fn dir_celda(out: &Path, cell: &str) -> PathBuf {
    out.join(cell)
}

/// R1-H09: `--out` con hallazgos del formato anterior (sin directorio por
/// celda). No se tocan; se avisa para que no se confundan con los nuevos.
pub fn aviso_salida_vieja(out: &Path) -> Option<String> {
    let viejos: Vec<&str> = ["div", "known", "panic", "lint", "asserts", "descartes.tsv", "known-sintomas.tsv"]
        .into_iter()
        .filter(|n| out.join(n).exists())
        .collect();
    (!viejos.is_empty()).then(|| {
        format!(
            "{} tiene {} de un difuzz anterior (sin directorio por celda): no los toco ni los uso; los hallazgos van a {}/<celda>/",
            out.display(),
            viejos.join(", "),
            out.display()
        )
    })
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

pub(crate) fn write_failed() {
    WRITE_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    WRITE_ERRORS_HILO.with(|c| c.set(c.get() + 1));
}

// 10.2b (B-H8 de la revision B del bloque 10): la misma cuenta por hilo, para los tests: el
// contador global lo suben tambien otros tests que corren en paralelo, y un `>` sobre el global
// podia pasar aunque la llamada que se prueba no contara.
#[cfg(test)]
thread_local! {
    static WRITE_ERRORS_HILO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn write_errors_hilo() -> usize {
    WRITE_ERRORS_HILO.with(std::cell::Cell::get)
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

    /// 10.1: el crudo y A son el mismo programa si el status y el stdout coinciden, salvo las
    /// posiciones de los marcos de un archivo de difuzz; los nombres de `util.inspect` no se borran.
    #[test]
    fn mismo_programa_sin_posiciones_y_con_nombres() {
        use super::mismo_programa;
        use crate::runner::{Exec, Status};
        let e = |status: Status, out: &str| Exec {
            status,
            stdout: out.into(),
            stderr: String::new(),
            flags: vec![],
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let marco = |p: &str| format!("Error: x\n    at f (/tmp/difuzz-7-w0.cjs:{p})\n");
        assert!(mismo_programa(&e(Status::Exited(0), &marco("1:20")), &e(Status::Exited(0), &marco("1:9"))));
        assert!(!mismo_programa(&e(Status::Exited(0), "\"\"\n"), &e(Status::Exited(0), "\"h\"\n")));
        assert!(!mismo_programa(&e(Status::Exited(0), "[Function (anonymous)]\n"), &e(Status::Exited(0), "[Function: h]\n")));
        assert!(!mismo_programa(&e(Status::Exited(0), "1\n"), &e(Status::Exited(1), "1\n")));
        // Un `archivo:L:C` impreso que no es de un marco real conserva la posicion.
        assert!(!mismo_programa(&e(Status::Exited(0), "    at f (x.js:1:2)\n"), &e(Status::Exited(0), "    at f (x.js:1:3)\n")));
    }

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

    /// 9.1 (H3 de la revision del bloque 8): con los nombres crudos (keep_names en la celda),
    /// `[Function: handler]` contra `[Function: e]` diverge; sin ellos (default del hilo, y
    /// core_defaults) se borran como antes. El modo vuelve al anterior al soltar la guarda.
    #[test]
    fn nombres_crudos_por_celda() {
        use super::{compare, con_nombres_crudos, nombres_de_celda, normalizadas, solo_nombres_de_inspect, symptom, Exec, Kind, Status, Verdict};
        let ex = |out: &str| Exec {
            status: Status::Exited(0),
            stdout: out.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let same = |a: &str, b: &str| matches!(compare(&ex(a), &ex(b)), Verdict::Same);
        let pares = [
            ("[Function: handler]\n", "[Function: e]\n"),
            ("[class Handler]\n", "[class e]\n"),
            ("[Function: bound handler]\n", "[Function: bound e]\n"),
            ("[Function: #handler]\n", "[Function: #e]\n"),
            ("[AsyncFunction: h] { a: 1 }\n", "[AsyncFunction: e] { a: 1 }\n"),
            ("[class B extends A]\n", "[class B extends e]\n"),
            ("[Function: f]\n", "[Function (anonymous)]\n"),
        ];
        for (a, b) in pares {
            assert!(same(a, b), "borrados: {a:?}");
        }
        {
            let _g = con_nombres_crudos(true);
            for (a, b) in pares {
                assert!(matches!(compare(&ex(a), &ex(b)), Verdict::Divergence(Kind::Stdout)), "crudos: {a:?}");
                assert!(solo_nombres_de_inspect(&ex(a), &ex(b)), "{a:?}");
                // solo_nombres_de_inspect no cambia el modo del hilo.
                assert!(!same(a, b), "crudos despues de solo_nombres_de_inspect: {a:?}");
                let (na, nb) = normalizadas(&ex(a), &ex(b));
                assert_ne!(na, nb);
                assert_ne!(symptom(Kind::Stdout, &ex(a), &ex("x\n")), symptom(Kind::Stdout, &ex(b), &ex("x\n")));
            }
            assert!(same("[Function: f]\n", "[Function: f]\n"));
            assert_eq!(nombres_de_celda("[Function: f]"), "[Function: f]");
            // Una diferencia que no es de nombres no es "solo nombres".
            assert!(!solo_nombres_de_inspect(&ex("[Function: f]\n1\n"), &ex("[Function: e]\n2\n")));
            {
                let _h = con_nombres_crudos(false);
                assert!(same("[Function: handler]\n", "[Function: e]\n"), "anidada");
            }
            assert!(!same("[Function: handler]\n", "[Function: e]\n"), "la anidada restaura crudos");
        }
        assert!(same("[Function: handler]\n", "[Function: e]\n"), "la guarda restaura el default");
        assert_eq!(nombres_de_celda("[Function: f]"), "[Function]");
        assert_eq!(symptom(Kind::Stdout, &ex("[Function: a]\n"), &ex("x\n")), symptom(Kind::Stdout, &ex("[Function: b]\n"), &ex("x\n")));
        // Una funcion sigue sin ser igual a un numero, en los dos modos.
        assert!(!same("[Function: f]\n", "6\n"));
        let _g = con_nombres_crudos(true);
        assert!(!same("[Function: f]\n", "6\n"));
        assert!(!solo_nombres_de_inspect(&ex("[Function: f]\n"), &ex("6\n")));
    }

    /// n-ruido (t-real): las posiciones de los marcos se borran; el nombre, la cantidad y
    /// el orden de los marcos no.
    #[test]
    fn stack_sin_posiciones() {
        // Como en `compare`, con los archivos que el preload habria anotado (7.5).
        const REALES: [&str; 6] = ["node:internal/x", "file:///tmp/d-w0.mjs", "/a.js", "<anonymous>", "/tmp/w2.cjs", "/a"];
        let n = |s: &str| super::normalize_stack_positions(&n(s), &REALES);
        let a ="TypeError: x\n    at cli (/tmp/difuzz-1-tmp/difuzz-1-w0.cjs:2:1016)\n    at /tmp/difuzz-1-tmp/difuzz-1-w0.cjs:2:929\n    at new A (node:internal/x:12:3)\n    at async Promise.all (index 0)\n";
        let b = "TypeError: x\n    at cli (/tmp/difuzz-1-tmp/difuzz-1-w0.cjs:2:658)\n    at /tmp/difuzz-1-tmp/difuzz-1-w0.cjs:2:835\n    at new A (node:internal/x:12:3)\n    at async Promise.all (index 0)\n";
        assert_eq!(n(a), n(b));
        assert_eq!(n(a), "TypeError: x\n    at cli (<pos>)\n    at <pos>\n    at new A (<pos>)\n    at async Promise.all (index 0)\n");
        // Carril B (file://) y eval.
        assert_eq!(n("    at f (file:///tmp/d-w0.mjs:1:407)"), "    at f (<pos>)");
        assert_eq!(n("    at eval (eval at g (/a.js:1:2), <anonymous>:1:5)"), "    at eval (eval at g (<pos>), <pos>)");
        // Un stack guardado en un string sale en JSON, en una linea.
        let j = |c: u32| format!("x = \"Error: e\\n    at serializeError (/tmp/w2.cjs:1:{c})\\n    at /tmp/w2.cjs:1:{}\"", c + 9);
        assert_eq!(n(&j(218)), n(&j(75)));
        assert_eq!(n(&j(218)), "x = \"Error: e\\n    at serializeError (<pos>)\\n    at <pos>\"");
        // Lo que no es posicion sigue: otro nombre, otro marco, otro orden, otra cabecera.
        assert_ne!(n("    at cli (/a.js:1:2)"), n("    at s (/a.js:1:2)"));
        assert_ne!(n("E\n    at f (/a.js:1:2)\n"), n("E\n    at f (/a.js:1:2)\n    at g (/a.js:3:4)\n"));
        assert_ne!(n("    at f (/a:1:2)\n    at g (/a:3:4)\n"), n("    at g (/a:1:2)\n    at f (/a:3:4)\n"));
        assert_ne!(n("TypeError: x\n    at f (/a.js:1:2)"), n("RangeError: x\n    at f (/a.js:1:2)"));
        // Solo marcos: texto con "at" en medio de la linea, o sin `:L:C` al final, no se toca.
        assert_eq!(n("look at /a.js:1:2"), "look at /a.js:1:2");
        assert_eq!(n("    at x:1:2b"), "    at x:1:2b");
        assert_eq!(n("    at 10:20"), "    at 10:20");
    }

    /// 7.5 (revision n2, MENOR 5, repro `revision-n2/ruido/t03-stack-pos`): una linea
    /// impresa con forma de marco conserva la posicion si su archivo no es el de un marco
    /// real (anotado por el preload, o de difuzz); un marco real la pierde igual que antes.
    #[test]
    fn marco_impreso_conserva_la_posicion() {
        use super::{compare, normalize_stack_positions as nsp, Verdict, MARCO};
        use crate::runner::{Exec, Status};
        // El repro: `pos` sale de un `substring` que el bug cambia (2 -> 3).
        let fake = |p: u32| format!("    at main (input.js:{p}:{})\n", p + 1);
        assert_eq!(nsp(&fake(2), &[]), fake(2));
        assert_ne!(nsp(&fake(2), &["/tmp/difuzz-1-tmp/difuzz-1-w0.cjs"]), nsp(&fake(3), &["/tmp/difuzz-1-tmp/difuzz-1-w0.cjs"]));
        // Los archivos de difuzz no necesitan anotacion (un stack que no paso por el preload):
        // el programa en los dos carriles, el envoltorio y el preload. Otro `difuzz-` no.
        for f in ["/tmp/difuzz-1-tmp/difuzz-1-w0.cjs", "file:///tmp/difuzz-1-tmp/difuzz-1-w0.mjs", "/tmp/x/difuzz-9-w3-observar.mjs", "/tmp/difuzz-9-preload.cjs"] {
            assert_eq!(nsp(&format!("    at f ({f}:1:2)"), &[]), "    at f (<pos>)", "{f}");
        }
        assert_eq!(nsp("    at f (/a/difuzz-x-w0.cjs:1:2)", &[]), "    at f (/a/difuzz-x-w0.cjs:1:2)");
        assert_eq!(nsp("    at f (difuzz-12:1:2)", &[]), "    at f (difuzz-12:1:2)");
        // Anotado por el preload (un `//# sourceURL=foo.js`, MEDIDO node 24: los marcos del
        // programa salen con ese nombre), si.
        assert_eq!(nsp("    at f (foo.js:1:22)", &["foo.js"]), "    at f (<pos>)");
        // El limite de antes (`  at 10:12:30`): el archivo seria `10`, que no es de un marco.
        assert_eq!(nsp("  at 10:12:30", &[]), "  at 10:12:30");
        // Uno si y otro no en el mismo marco; el archivo no se extiende sobre la posicion
        // anterior aunque esa no se haya borrado.
        assert_eq!(nsp("    at eval (eval at g (/no.js:1:2), <anonymous>:1:5)", &["<anonymous>"]), "    at eval (eval at g (/no.js:1:2), <pos>)");
        assert_eq!(nsp("    at f (x:1:2,y:3:4)", &["y"]), "    at f (x:1:2,y:3:4)");
        assert_eq!(nsp("    at f (x:1:2, y:3:4)", &["y"]), "    at f (x:1:2, <pos>)");
        // En un string JSON, igual.
        assert_eq!(nsp("\"E\\n    at main (input.js:2:3)\"", &[]), "\"E\\n    at main (input.js:2:3)\"");
        // De punta a punta en `compare`: los archivos salen del canal de A o de B.
        let ex = |out: &str, ctl: &str| Exec {
            status: Status::Exited(0),
            stdout: out.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: ctl.into(),
        };
        let same = |a: &Exec, b: &Exec| matches!(compare(a, b), Verdict::Same);
        assert!(!same(&ex(&fake(2), "dz\n"), &ex(&fake(3), "dz\n")), "t03: el bug en la posicion impresa diverge");
        let src = |c: u32| format!("Error: e\n    at f (foo.js:1:{c})\n");
        let canal = format!("{MARCO}foo.js\n{MARCO}node:internal/x\nflags stack\n");
        assert!(!same(&ex(&src(22), ""), &ex(&src(9), "")));
        assert!(same(&ex(&src(22), &canal), &ex(&src(9), "")), "anotado en A");
        assert!(same(&ex(&src(22), ""), &ex(&src(9), &canal)), "anotado en B");
        // El programa (de difuzz) sin anotar: un stack que no paso por el preload.
        let prog = |c: u32| format!("Error: e\n    at f (/tmp/difuzz-7-tmp/difuzz-7-w1.cjs:1:{c})\n");
        assert!(same(&ex(&prog(22), ""), &ex(&prog(9), "")));
    }

    /// 7.5 con node: el preload anota los archivos de los marcos que formatea (el programa,
    /// `node:...`, un `//# sourceURL`, un `vm` con `filename`, el `eval`) y `compare` borra
    /// sus posiciones; el repro t03 (una linea impresa con forma de marco) diverge.
    #[test]
    fn marcos_reales_con_node() {
        use super::{compare, Verdict, MARCO};
        use crate::runner::{Preload, Runner};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("marcos").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let same = |x: &str, y: &str| {
            let (a, b) = (r.run(x), r.run(y));
            (matches!(compare(&a, &b), Verdict::Same), a)
        };
        // Solo cambian columnas (un espacio de mas, como un minificado que corre el codigo).
        let pila = |pad: &str| {
            format!(
                "function f() {{{pad}return new Error('e').stack; }}\nconsole.log(f());\neval(\"console.log(new Error('x').stack)\");\n\
                 console.log(require('vm').runInThisContext(\"new Error('v').stack\", {{ filename: 'vm-propio.js' }}));\n"
            )
        };
        // `//# sourceURL` (MEDIDO node 24: los marcos del programa salen con ese nombre).
        let (igual, a) = same(&format!("{}//# sourceURL=propio.js\n", pila("")), &format!("{}//# sourceURL=propio.js\n", pila("   ")));
        assert!(igual && a.stdout.contains("(propio.js:1:"), "{}", a.stdout);
        let (igual, a) = same(&pila(""), &pila("   "));
        assert!(igual, "{}", a.stdout);
        let anotados: Vec<&str> = a.ctl.lines().filter_map(|l| l.strip_prefix(MARCO)).collect();
        assert!(anotados.iter().any(|f| f.ends_with(".cjs") && f.contains("difuzz-")), "{anotados:?}");
        for f in ["<anonymous>", "vm-propio.js"] {
            assert!(anotados.contains(&f), "{f}: {anotados:?}");
        }
        assert!(anotados.iter().any(|f| f.starts_with("node:")), "{anotados:?}");
        // El repro t03: el bug cambia el numero impreso.
        let t03 = |fin: &str| format!("var pos = \"abc\".substring(1, {fin}).length;\nconsole.log(\"    at main (input.js:\" + pos + \":\" + (pos + 1) + \")\");\n");
        let (igual, a) = same(&t03("undefined"), &t03("2"));
        assert!(!igual, "t03: {}", a.stdout);
        assert_eq!(a.stdout, "    at main (input.js:2:3)\n");
        // El mismo programa leyendo de verdad un stack (anota el archivo del programa, no
        // `input.js`): la linea impresa sigue divergiendo.
        let (igual, _) = same(&format!("void new Error().stack;\n{}", t03("undefined")), &format!("void new Error().stack;\n{}", t03("2")));
        assert!(!igual);
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
        r.divergence(std::path::Path::new("c/input.js"), "o", "m", &a, &b, Kind::ThrowNew, false, None, None, &[], &[], "miss", "clasificacion:\n  decision: div/\n");
        let meta = std::fs::read_to_string(d.join("div/c/meta.txt")).unwrap();
        for want in ["a_cache: miss\n", "node: v0\n", "backend: x 1\n", "debug_assertions: ", "repro_original: cd /", "--no-warnings --require /", "original.cjs\n", "repro_minificado: cd /", "\nclasificacion:\n  decision: div/\n"] {
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

    /// R2-H08: el meta de lint/ nombra el archivo que se guardo (.mjs en carril B).
    #[test]
    fn lint_nombra_la_extension_del_carril() {
        use super::Reporter;
        use crate::minify::SourceKind;
        let d = std::env::temp_dir().join(format!("difuzz-test-lint-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (kind, ext, otra) in [(SourceKind::Module, "mjs", "cjs"), (SourceKind::Script, "cjs", "mjs")] {
            let r = Reporter::new(&d, "celda".into(), "//preload").with_kind(kind);
            r.lint(std::path::Path::new("c/input.js"), "o", "m", (0, 1), "-");
            let meta = std::fs::read_to_string(d.join("lint/c/meta.txt")).unwrap();
            assert!(meta.contains(&format!("buscar en minified.{ext} ")), "{meta}");
            assert!(!meta.contains(&format!("minified.{otra}")), "{meta}");
            assert!(d.join(format!("lint/c/minified.{ext}")).is_file());
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 10.4 (M1 de rev-b9-A): una linea de descartes.tsv que no se pudo escribir y un indice que
    /// no se pudo cerrar cuentan como escrituras perdidas (antes: solo stderr, `completa` y 0).
    #[test]
    fn descartes_e_indice_perdidos_cuentan() {
        use super::{write_errors_hilo, Reporter};
        let d = std::env::temp_dir().join(format!("difuzz-test-perdidas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ix = crate::indice::Index::create(&d.join("indice"), "celda").unwrap();
        let r = Reporter::new(&d, "celda".into(), "//preload").with_index(ix);
        // El .tmp del indice ya no esta: el rename del cierre falla.
        std::fs::remove_file(d.join("indice/index-celda.tsv.tmp")).unwrap();
        let antes = write_errors_hilo();
        r.finish(crate::indice::Cierre::Completa);
        assert_eq!(write_errors_hilo(), antes + 1, "el cierre fallido del indice no conto");
        if std::path::Path::new("/dev/full").exists() {
            let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
            *r.descartes.lock().unwrap() = Some(full);
            let antes = write_errors_hilo();
            r.discard(std::path::Path::new("c/input.js"), "timeout", std::time::Duration::ZERO);
            assert_eq!(write_errors_hilo(), antes + 1, "la linea perdida de descartes.tsv no conto");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 10.10 (A-H6 de la revision A de 10.1): el hash de un `codegen/` en el indice es el del
    /// sintoma con los nombres crudos, tambien en un hilo que los borra (core_defaults).
    #[test]
    fn sintoma_de_codegen_con_nombres_crudos() {
        use super::{con_nombres_crudos, symptom, Kind, Reporter};
        use crate::runner::{Exec, Status};
        let d = std::env::temp_dir().join(format!("difuzz-test-codegen-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ex = |out: &str| Exec { status: Status::Exited(0), stdout: out.into(), stderr: String::new(), flags: Vec::new(), elapsed: std::time::Duration::ZERO, ctl: String::new() };
        let (r, a) = (ex("[Function: h]\n"), ex("[Function (anonymous)]\n"));
        let ix = crate::indice::Index::create(&d.join("indice"), "celda").unwrap();
        let rep = Reporter::new(&d, "celda".into(), "//preload").with_index(ix);
        let _borrados = con_nombres_crudos(false);
        rep.codegen(std::path::Path::new("c/input.js"), "crudo", "orig", &r, &a, Kind::Stdout, "-");
        rep.finish(crate::indice::Cierre::Completa);
        let crudo = {
            let _c = con_nombres_crudos(true);
            symptom(Kind::Stdout, &r, &a)
        };
        assert_ne!(crudo, symptom(Kind::Stdout, &r, &a), "con los nombres borrados el sintoma tendria que ser otro");
        let tsv = std::fs::read_to_string(d.join("indice/index-celda.tsv")).unwrap();
        assert!(tsv.contains(&format!("\tcodegen\tStdout\t{crudo:016x}\n")), "{tsv}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn una_funcion_sigue_sin_ser_un_numero() {
        // inverted_var: [Function: c] contra 6 tiene que seguir divergiendo.
        assert_ne!(n("1 2 [Function: c] 7"), n("1 2 6 7"));
    }
}
