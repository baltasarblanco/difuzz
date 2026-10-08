//! MIN: el backend elegido (backend/swc.rs, backend/oxc.rs) parsea, imprime y
//! minifica.
//!
//! Devuelve DOS programas impresos con el mismo parser y el mismo printer:
//! el original sin optimizar y el minificado. Asi, una divergencia entre
//! ambos solo puede venir del optimizador del backend. Comparar contra el
//! texto fuente crudo mezclaria bugs de parser/codegen con bugs del minifier.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, SourceMap, GLOBALS};
use swc_ecma_ast::{Decl, EsVersion, ObjectPatProp, Pat, Stmt};
use swc_ecma_parser::{parse_file_as_script, Syntax};

use crate::backend::{Backend, Minifier};

/// Un minifier ya configurado: backend + preset.
pub struct Config(Box<dyn Backend>);

/// Primer eje de la matriz: con que opciones se minifica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    CarrilA,
    CoreDefaults,
}

impl Preset {
    pub const ALL: &'static str = "carril_a | core_defaults";

    pub fn name(self) -> &'static str {
        match self {
            Preset::CarrilA => "carril_a",
            Preset::CoreDefaults => "core_defaults",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        match s {
            "carril_a" => Some(Preset::CarrilA),
            "core_defaults" => Some(Preset::CoreDefaults),
            _ => None,
        }
    }

    /// La config del preset para un backend y un tipo de fuente. El carril
    /// (script o modulo) llega explicito: el backend lo recibe, no lo adivina.
    pub fn build(self, minifier: Minifier, kind: SourceKind) -> Config {
        Config(minifier.build(self, kind))
    }
}

/// Segundo eje de la matriz: la forma del programa que se minifica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forma {
    TalCual,
    EnFuncion,
}

impl Forma {
    pub const ALL: &'static str = "tal_cual | en_funcion";

    pub fn name(self) -> &'static str {
        match self {
            Forma::TalCual => "tal_cual",
            Forma::EnFuncion => "en_funcion",
        }
    }

    pub fn parse(s: &str) -> Option<Forma> {
        match s {
            "tal_cual" => Some(Forma::TalCual),
            "en_funcion" => Some(Forma::EnFuncion),
            _ => None,
        }
    }

    /// `en_funcion` es el wrapper de probe/probe.cjs, caracter por caracter: el
    /// codigo real vive casi todo dentro de funciones (modulos, wrappers de
    /// bundlers), y ahi el DCE corre sin `top_level`. Es la forma que mostro uso
    /// comun en swc#12397 y swc#12400. Se aplica DESPUES del epilogo observador,
    /// para que el epilogo quede dentro de la funcion y vea los bindings.
    pub fn apply(self, src: &str) -> String {
        match self {
            Forma::TalCual => src.to_string(),
            Forma::EnFuncion => format!("(function () {{\n{src}\n}})();\n"),
        }
    }
}

/// Tercer eje: como se parsea y se ejecuta el fuente. Carril A es script +
/// CommonJS (`.cjs`, sloppy); carril B es modulo ES (`.mjs`, estricto, `this`
/// top-level `undefined`, `import`/`export`). Llega explicito hasta `prepare`:
/// el backend del minifier lo recibe como parametro, no lo adivina.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Script,
    Module,
}

impl SourceKind {
    pub const ALL: &'static str = "a | b";

    /// Nombre del carril en la CLI (`--carril a|b`).
    pub fn carril(self) -> &'static str {
        match self {
            SourceKind::Script => "a",
            SourceKind::Module => "b",
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "a" => Some(SourceKind::Script),
            "b" => Some(SourceKind::Module),
            _ => None,
        }
    }

    /// Extension con la que node decide como ejecutar el archivo.
    pub fn ext(self) -> &'static str {
        match self {
            SourceKind::Script => "cjs",
            SourceKind::Module => "mjs",
        }
    }
}

impl Config {
    /// core_defaults con los nombres preservados (keep_* en compress y mangle).
    /// No es una celda: sirve para preguntar si una divergencia de core_defaults
    /// desaparece cuando los nombres no cambian (main::names_only).
    pub fn core_defaults_keep_names(minifier: Minifier, kind: SourceKind) -> Self {
        Config(minifier.build_core_defaults_keep_names(kind))
    }
}

pub enum Outcome {
    Ok {
        original: String,
        minified: String,
        /// checks::static_flags sobre el original.
        static_flags: Vec<&'static str>,
        /// checks::dead_writes (original, minificado).
        dead_writes: (usize, usize),
    },
    /// El parser rechazo el fuente o lo acepto con errores recuperados.
    /// `esm`: en carril A, parsea como modulo (es del carril B); solo se cuenta.
    /// `script`: en carril B, parsea como script (es del carril A); el espejo.
    ParseError { esm: bool, script: bool },
    CodegenError,
    /// `optimize` (o algo en la cadena) entro en panico. Es un hallazgo.
    Panic(String),
}

pub fn prepare(src: &str, cfg: &Config) -> Outcome {
    let result = catch_unwind(AssertUnwindSafe(|| cfg.0.prepare(src)));
    match result {
        Ok(outcome) => outcome,
        Err(payload) => Outcome::Panic(panic_message(&payload)),
    }
}

/// Maximo de bindings que observa el epilogo. Mas no agrega senal y alarga la salida.
/// Si hay mas, el epilogo lo dice (`!dz truncados = N`): el recorte no es silencioso.
const MAX_OBSERVED: usize = 30;

/// Epilogo observador: devuelve `src` con una llamada al final que imprime sus
/// bindings top-level, o `None` si no declara ninguno.
///
/// Solo se usa para programas que no imprimen nada: sin salida, el oraculo es
/// ciego (el fixture 9739 congelo un bug asi). Va en el fuente ANTES de
/// minificar, asi el minifier tiene que preservar los valores observados.
/// `__difuzzObserve` lo define el preload, que no se minifica.
///
/// Costo: los bindings observados dejan de ser "no usados", y el DCE sobre
/// ellos no corre. Por eso no se aplica a programas que ya imprimen.
///
/// `invocar` (mejora 7, `--invocar`): el epilogo llama a `__difuzzObserveInvocar`,
/// que imprime lo mismo y despues llama a cada funcion observada (preload.js).
pub fn instrument(src: &str, invocar: bool) -> Option<String> {
    let names = catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || top_level_bindings(src))
    }))
    .ok()??;
    if names.is_empty() {
        return None;
    }
    let mut args: Vec<String> = names
        .iter()
        .take(MAX_OBSERVED)
        .map(|n| format!("[\"{n}\", {n}]"))
        .collect();
    if names.len() > MAX_OBSERVED {
        args.push(format!("[\"!dz truncados\", {}]", names.len() - MAX_OBSERVED));
    }
    // "\n;\n": cierra un comentario de linea final y evita que ASI pegue el
    // epilogo a la ultima expresion del programa.
    let observe = if invocar { "__difuzzObserveInvocar" } else { "__difuzzObserve" };
    Some(format!("{src}\n;\n{observe}({});\n", args.join(", ")))
}

fn top_level_bindings(src: &str) -> Option<Vec<String>> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    let script = parse_file_as_script(
        &fm,
        Syntax::default(),
        EsVersion::latest(),
        None,
        &mut Vec::new(),
    )
    .ok()?;

    let mut names = Vec::new();
    for stmt in &script.body {
        let Stmt::Decl(decl) = stmt else {
            continue;
        };
        match decl {
            Decl::Var(var) => {
                for d in &var.decls {
                    collect_pat(&d.name, &mut names);
                }
            }
            Decl::Fn(f) => push_name(&mut names, f.ident.sym.as_str()),
            Decl::Class(c) => push_name(&mut names, c.ident.sym.as_str()),
            _ => {}
        }
    }
    Some(names)
}

fn collect_pat(pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(b) => push_name(out, b.id.sym.as_str()),
        Pat::Array(a) => {
            for p in a.elems.iter().flatten() {
                collect_pat(p, out);
            }
        }
        Pat::Rest(r) => collect_pat(&r.arg, out),
        Pat::Object(o) => {
            for prop in &o.props {
                #[allow(unreachable_patterns)]
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat(&kv.value, out),
                    ObjectPatProp::Assign(a) => push_name(out, a.key.id.sym.as_str()),
                    ObjectPatProp::Rest(r) => collect_pat(&r.arg, out),
                    _ => {}
                }
            }
        }
        Pat::Assign(a) => collect_pat(&a.left, out),
        _ => {}
    }
}

fn push_name(out: &mut Vec<String>, name: &str) {
    if !out.iter().any(|n| n == name) {
        out.push(name.to_string());
    }
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic sin mensaje".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{Forma, Preset};
    use crate::backend::swc::Swc;

    #[test]
    fn ejes_de_la_matriz() {
        for p in [Preset::CarrilA, Preset::CoreDefaults] {
            assert_eq!(Preset::parse(p.name()), Some(p));
        }
        for f in [Forma::TalCual, Forma::EnFuncion] {
            assert_eq!(Forma::parse(f.name()), Some(f));
        }
        assert_eq!(Preset::parse("otro"), None);
        // El mismo wrapper que probe/probe.cjs.
        assert_eq!(Forma::EnFuncion.apply("x;"), "(function () {\nx;\n})();\n");
        assert_eq!(Forma::TalCual.apply("x;"), "x;");
        // core_defaults: sin top_level ni keep_* en mangle (a diferencia de carril_a).
        let m = Swc::core_defaults().minify.mangle.unwrap();
        assert_ne!(m.top_level, Some(true));
        assert!(!m.keep_fn_names && !m.keep_class_names);
    }
}
