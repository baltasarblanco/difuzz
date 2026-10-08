//! MIN: el backend elegido (backend/swc.rs, backend/oxc.rs) parsea, imprime y
//! minifica.
//!
//! Devuelve DOS programas impresos con el mismo parser y el mismo printer:
//! el original sin optimizar y el minificado. Asi, una divergencia entre
//! ambos solo puede venir del optimizador del backend. Comparar contra el
//! texto fuente crudo mezclaria bugs de parser/codegen con bugs del minifier.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{Globals, GLOBALS};
use swc_ecma_ast::{Decl, ModuleDecl, ModuleItem, ObjectPatProp, Pat, Program, Stmt};

use crate::backend::{Backend, Minifier};

/// Un minifier ya configurado: backend + preset. El segundo campo (R1-H05) es
/// el oraculo sin debug assertions (subproceso.rs): si el primero hace panic,
/// `prepare` re-minifica con el.
pub struct Config(Box<dyn Backend>, Option<Box<dyn Backend>>);

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
        let oraculo = crate::subproceso::oraculo(minifier, self, kind, false);
        // M34: salvo --en-proceso, la misma config pero minificando en un hijo.
        if crate::subproceso::activo() {
            return Config(Box::new(crate::subproceso::Remoto::new(minifier, self, kind, false)), oraculo);
        }
        Config(minifier.build(self, kind), oraculo)
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
        let oraculo = crate::subproceso::oraculo(minifier, Preset::CoreDefaults, kind, true);
        if crate::subproceso::activo() {
            return Config(Box::new(crate::subproceso::Remoto::new(minifier, Preset::CoreDefaults, kind, true)), oraculo);
        }
        Config(minifier.build_core_defaults_keep_names(kind), oraculo)
    }

    /// M34: una config sobre un backend ya armado (el hijo de `--preparar`).
    /// Sin oraculo: el hijo no re-minifica.
    pub fn from_backend(backend: Box<dyn crate::backend::Backend>) -> Self {
        Config(backend, None)
    }

    /// R1-H05, tests: una config con un oraculo a mano.
    #[cfg(test)]
    pub fn con_oraculo(backend: Box<dyn Backend>, oraculo: Box<dyn Backend>) -> Self {
        Config(backend, Some(oraculo))
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
    match (prepare_con(&*cfg.0, src), &cfg.1) {
        (Outcome::Panic(msg), Some(oraculo)) => respaldo(src, msg, &**oraculo),
        (o, _) => o,
    }
}

fn prepare_con(backend: &dyn Backend, src: &str) -> Outcome {
    PANIC_LOCATION.with(|l| l.borrow_mut().take());
    let result = catch_unwind(AssertUnwindSafe(|| backend.prepare(src)));
    match result {
        Ok(outcome) => outcome,
        Err(payload) => {
            let mut msg = panic_message(&payload);
            // M22: de que crate viene (parser, minifier o codegen).
            if let Some(loc) = PANIC_LOCATION.with(|l| l.borrow_mut().take()) {
                msg.push_str(&format!("\nubicacion: {loc}"));
            }
            Outcome::Panic(msg)
        }
    }
}

/// R1-H05: el backend con debug assertions hizo panic (`msg`); se minifica de
/// nuevo con el oraculo sin debug assertions (lo que corre npm).
/// - Devuelve codigo: el panic era solo de los asserts. Se usa ese minificado
///   (el caso se compara como cualquiera) y el panic queda en `NOTA_ASSERTS`.
/// - Tambien falla: es un panic real; el mensaje lo dice.
/// - No parsea / no imprime: raro (el assert salto antes); se conserva el panic
///   con la nota, para no esconder nada.
fn respaldo(src: &str, msg: String, oraculo: &dyn Backend) -> Outcome {
    match prepare_con(oraculo, src) {
        Outcome::Ok { original, minified, static_flags, dead_writes } => {
            NOTA_ASSERTS.with(|n| {
                n.borrow_mut().get_or_insert(NotaAsserts { panic: msg, minificado: minified.clone() });
            });
            Outcome::Ok { original, minified, static_flags, dead_writes }
        }
        Outcome::Panic(m2) if m2 == msg => {
            Outcome::Panic(format!("{msg}\noraculo sin debug assertions: el mismo panic (es real, no solo de un debug_assert)"))
        }
        Outcome::Panic(m2) => Outcome::Panic(format!("{msg}\noraculo sin debug assertions: tambien falla (es real):\n{m2}")),
        Outcome::ParseError { .. } => Outcome::Panic(format!("{msg}\noraculo sin debug assertions: no parsea el fuente")),
        Outcome::CodegenError => Outcome::Panic(format!("{msg}\noraculo sin debug assertions: error de codegen")),
    }
}

/// R1-H05: la primera vez en el caso en curso que el binario con debug
/// assertions hizo panic y el oraculo minifico. La escribe `prepare`; main la
/// limpia al empezar cada caso (`tomar_nota_asserts`) y el Reporter la lee al
/// escribir div/known. Puede venir de otra pasada del mismo caso (observador,
/// re-chequeo): por eso guarda el minificado, para saber a que programa aplica.
#[derive(Clone, Debug)]
pub struct NotaAsserts {
    /// El mensaje del panic con debug assertions (con la ubicacion).
    pub panic: String,
    /// Lo que devolvio el oraculo.
    pub minificado: String,
}

pub fn nota_asserts() -> Option<NotaAsserts> {
    NOTA_ASSERTS.with(|n| n.borrow().clone())
}

/// R1-H05: devuelve la nota y la borra.
pub fn tomar_nota_asserts() -> Option<NotaAsserts> {
    NOTA_ASSERTS.with(|n| n.borrow_mut().take())
}

thread_local! {
    /// M22: ubicacion del ultimo panic en este hilo (la escribe el hook de main).
    static PANIC_LOCATION: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// R1-H05: ver `nota_asserts`. Por hilo: cada worker procesa un caso a la vez.
    static NOTA_ASSERTS: std::cell::RefCell<Option<NotaAsserts>> = const { std::cell::RefCell::new(None) };
}

/// M22: llamado desde el panic hook. Corre en el hilo que entro en panico.
pub fn record_panic_location(info: &std::panic::PanicHookInfo) {
    if let Some(l) = info.location() {
        let loc = format!("{}:{}:{}", l.file(), l.line(), l.column());
        PANIC_LOCATION.with(|p| *p.borrow_mut() = Some(loc));
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
/// M31: via `globalThis[...]`, y despues `__difuzzObserveDespues` re-observa los
/// bindings no-funcion que cambiaron.
///
/// `kind` (M12): los bindings se leen parseando segun el carril; en el carril B
/// es el epilogo de un modulo sin exports.
pub fn instrument(src: &str, invocar: bool, kind: SourceKind) -> Option<String> {
    let names = catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || top_level_bindings(src, kind))
    }))
    .ok()??;
    // M36 (decision del usuario, opcion a): en el carril A (CommonJS) un programa
    // que menciona `module`/`exports` (UMD, CJS) puede no tener bindings top-level
    // y exportar todo por `module.exports`. Se observa eso tambien. `module` es la
    // variable del envoltorio CommonJS de node: con `en_funcion` sigue a la vista
    // (la funcion esta adentro del .cjs), salvo que el programa declare su propio
    // `module` top-level, que ya se observa como binding.
    let cjs = kind == SourceKind::Script
        && (mentions_word(src, "module") || mentions_word(src, "exports"))
        && !names.iter().any(|n| n == "module");
    if names.is_empty() && !cjs {
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
    // M31 (b): si el programa declara `__difuzzObserveInvocar` top-level, se
    // llama via `globalThis[...]`: su binding (CJS, modulo o `en_funcion`:
    // siempre local) ya no la sombrea. Si ademas declara `globalThis`, no hay por
    // donde llamar: sin invocar. Sin colision, el nombre suelto de siempre: la
    // forma `globalThis[...]` cambia lo que hace el minifier (MEDIDO: con ella
    // SWC dejaba de borrar la `function foo` de unused_keep_harmony_destructuring
    // y se tapaba esa divergencia).
    let declared = |n: &str| names.iter().any(|x| x == n);
    let invocar = invocar && !(declared("__difuzzObserveInvocar") && declared("globalThis"));
    let via_global = |f: &str| if declared(f) { format!("globalThis[\"{f}\"]") } else { f.to_string() };
    let observe = if invocar { via_global("__difuzzObserveInvocar") } else { "__difuzzObserve".to_string() };
    let mut out = format!("{src}\n;\n");
    if !args.is_empty() {
        out.push_str(&format!("{observe}({});\n", args.join(", ")));
        // M31 (c): los mismos bindings leidos de nuevo despues de invocar (un
        // primitivo reasignado por una funcion se ve recien aca). Sin ella si el
        // nombre esta tomado y `globalThis` tambien.
        if invocar && !(declared("__difuzzObserveDespues") && declared("globalThis")) {
            let vars = catch_unwind(AssertUnwindSafe(|| GLOBALS.set(&Globals::new(), || var_bindings(src, kind)))).unwrap_or_default();
            let again: Vec<String> = names
                .iter()
                .take(MAX_OBSERVED)
                .filter(|n| vars.contains(n))
                .map(|n| format!("[\"{n}\", {n}]"))
                .collect();
            if !again.is_empty() {
                out.push_str(&format!("{}({});\n", via_global("__difuzzObserveDespues"), again.join(", ")));
            }
        }
    }
    if cjs {
        out.push_str(&format!("__difuzzObserveCjs(module.exports, {invocar});\n"));
    }
    Some(out)
}

/// `word` aparece en `src` como palabra (no como parte de un identificador).
/// Aproximado a proposito: puede ser un comentario o un string; el costo es
/// observar un `module.exports` vacio (`{}`).
fn mentions_word(src: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    src.match_indices(word).any(|(i, _)| {
        !src[..i].chars().next_back().is_some_and(ident) && !src[i + word.len()..].chars().next().is_some_and(ident)
    })
}

fn top_level_bindings(src: &str, kind: SourceKind) -> Option<Vec<String>> {
    let program = crate::backend::swc::parse_for(src, kind)?;
    let decls: Vec<&Decl> = match &program {
        Program::Script(s) => s
            .body
            .iter()
            .filter_map(|st| match st {
                Stmt::Decl(d) => Some(d),
                _ => None,
            })
            .collect(),
        Program::Module(m) => m
            .body
            .iter()
            .filter_map(|it| match it {
                ModuleItem::Stmt(Stmt::Decl(d)) => Some(d),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => Some(&e.decl),
                _ => None,
            })
            .collect(),
    };

    let mut names = Vec::new();
    for decl in decls {
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

/// M31 (c): solo los bindings `var`/`let`/`const` top-level (los que una funcion
/// invocada puede reasignar). Las `function`/`class` no se re-observan: una
/// segunda referencia en el epilogo le cambia al minifier lo que puede borrar
/// (MEDIDO: tapaba la divergencia de unused_keep_harmony_destructuring).
fn var_bindings(src: &str, kind: SourceKind) -> Vec<String> {
    let Some(program) = crate::backend::swc::parse_for(src, kind) else {
        return Vec::new();
    };
    let vars: Vec<&swc_ecma_ast::VarDecl> = match &program {
        Program::Script(s) => s
            .body
            .iter()
            .filter_map(|st| match st {
                Stmt::Decl(Decl::Var(v)) => Some(&**v),
                _ => None,
            })
            .collect(),
        Program::Module(m) => m
            .body
            .iter()
            .filter_map(|it| match it {
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(v))) => Some(&**v),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => match &e.decl {
                    Decl::Var(v) => Some(&**v),
                    _ => None,
                },
                _ => None,
            })
            .collect(),
    };
    let mut names = Vec::new();
    for v in vars {
        for d in &v.decls {
            collect_pat(&d.name, &mut names);
        }
    }
    names
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

    /// M22: un panic adentro de prepare sale con su ubicacion.
    #[test]
    fn panic_con_ubicacion() {
        use super::{prepare, record_panic_location, Config, Outcome};
        use crate::backend::Backend;
        struct Revienta;
        impl Backend for Revienta {
            fn prepare(&self, _: &str) -> Outcome {
                panic!("adentro del backend")
            }
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(record_panic_location));
        let out = prepare("x", &Config(Box::new(Revienta), None));
        std::panic::set_hook(prev);
        let Outcome::Panic(msg) = out else { panic!("tenia que ser Panic") };
        assert!(msg.starts_with("adentro del backend\nubicacion: src/minify.rs:"), "{msg}");
    }

    /// R1-H05: ante un panic, el oraculo decide. Si minifica, el caso sigue con
    /// su minificado y el panic queda en la nota; si tambien falla, es panic real
    /// y el mensaje lo dice; sin panic, el oraculo ni se consulta.
    #[test]
    fn oraculo_decide_ante_un_panic() {
        use super::{nota_asserts, prepare, tomar_nota_asserts, Config, Outcome};
        use crate::backend::Backend;
        struct Assert;
        impl Backend for Assert {
            fn prepare(&self, src: &str) -> Outcome {
                if src.contains("eval") {
                    panic!("assertion failed: solo con debug assertions")
                }
                Outcome::Ok { original: src.into(), minified: src.into(), static_flags: Vec::new(), dead_writes: (0, 0) }
            }
        }
        struct Npm;
        impl Backend for Npm {
            fn prepare(&self, src: &str) -> Outcome {
                Outcome::Ok { original: src.into(), minified: "npm".into(), static_flags: vec!["with"], dead_writes: (1, 2) }
            }
        }
        struct Real;
        impl Backend for Real {
            fn prepare(&self, _: &str) -> Outcome {
                Outcome::Panic("byte index 1 is out of range".into())
            }
        }
        tomar_nota_asserts();
        let cfg = Config::con_oraculo(Box::new(Assert), Box::new(Npm));
        let Outcome::Ok { minified, static_flags, dead_writes, .. } = prepare("x", &cfg) else { panic!() };
        assert_eq!((minified.as_str(), dead_writes), ("x", (0, 0)), "sin panic no se consulta al oraculo");
        assert!(static_flags.is_empty() && nota_asserts().is_none());
        let Outcome::Ok { minified, static_flags, dead_writes, .. } = prepare("var e = eval;", &cfg) else { panic!() };
        assert_eq!((minified.as_str(), static_flags, dead_writes), ("npm", vec!["with"], (1, 2)));
        let nota = tomar_nota_asserts().expect("nota del assert");
        assert!(nota.panic.starts_with("assertion failed: solo con debug assertions"), "{nota:?}");
        assert_eq!(nota.minificado, "npm");
        assert!(nota_asserts().is_none());
        // Panic real: el oraculo tambien falla.
        let cfg = Config::con_oraculo(Box::new(Assert), Box::new(Real));
        let Outcome::Panic(msg) = prepare("eval", &cfg) else { panic!() };
        assert!(msg.starts_with("assertion failed") && msg.contains("\noraculo sin debug assertions: tambien falla (es real):\nbyte index"), "{msg}");
        let cfg = Config::con_oraculo(Box::new(Real), Box::new(Real));
        let Outcome::Panic(msg) = prepare("x", &cfg) else { panic!() };
        assert_eq!(msg, "byte index 1 is out of range\noraculo sin debug assertions: el mismo panic (es real, no solo de un debug_assert)");
        assert!(nota_asserts().is_none());
    }

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
