//! MIN: la cadena parse → resolver → optimize → fixer → codegen.
//!
//! Devuelve DOS programas impresos con el mismo parser y el mismo printer:
//! el original sin optimizar y el minificado. Asi, una divergencia entre
//! ambos solo puede venir de `optimize` (mas resolver/fixer, que corren en
//! los dos lados). Comparar contra el texto fuente crudo mezclaria bugs de
//! parser/codegen con bugs del minifier.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, Mark, SourceFile, SourceMap, GLOBALS};
use swc_ecma_ast::{Decl, EsVersion, ObjectPatProp, Pat, Program, Stmt};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_minifier::{
    optimize,
    option::{
        terser::TerserCompressorOptions, ExtraOptions, MangleOptions, MinifyOptions,
        TopLevelOptions,
    },
};
use swc_ecma_parser::{parse_file_as_module, parse_file_as_script, Syntax};
use swc_ecma_transforms_base::{
    fixer::{fixer, paren_remover},
    resolver,
};

pub struct Config {
    pub minify: MinifyOptions,
}

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

    pub fn build(self) -> Config {
        match self {
            Preset::CarrilA => Config::carril_a(),
            Preset::CoreDefaults => Config::core_defaults(),
        }
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

impl Config {
    /// Carril A: script + CommonJS (delta v1.2, decision #1).
    ///
    /// La base de compresion se construye por el mismo camino que @swc/core:
    /// `TerserCompressorOptions` vacio → `into_config`. Asi los defaults son los
    /// del producto por construccion (`CompressOptions::default()` de Rust no:
    /// deja `reduce_vars: false`, y desde JS hereda `defaults = true`). Encima
    /// van solo las decisiones del carril.
    ///
    /// - `compress.module = false`: el minifier asume sloppy, igual que Node
    ///   al ejecutar un `.cjs`.
    /// - `compress.top_level = Some(..)`: sin esto `top_level()` da false y el
    ///   DCE top-level no corre; swc#12029 y swc#11963 son invisibles
    ///   (lecciones-pr-12029 §5).
    /// - `mangle.eval` queda en su default `false`: en el codigo es
    ///   `ignore_eval`, un opt-in que asume que ningun eval referencia
    ///   nombres locales. Prenderlo fabrica divergencias esperadas.
    /// - `mangle.props` queda en `None` en F1: el corpus imprime objetos
    ///   libremente y el mangling de propiedades es observable via reflexion.
    ///   Se prende en F3, cuando el generador controla la salida.
    /// - `keep_fnames`/`keep_classnames` (compress) y `keep_fn_names`/
    ///   `keep_class_names` (mangle) en `true`: sin eso, `console.log(f)` imprime
    ///   `[Function: f]` contra `[Function: n]` y es divergencia esperada. En la
    ///   primera corrida fueron 14 de las 32 divergencias en main.
    pub fn carril_a() -> Self {
        // into_config solo usa el SourceMap para parsear pure_funcs / global_defs,
        // que aca vienen vacios.
        let cm: Lrc<SourceMap> = Default::default();
        let mut compress = TerserCompressorOptions::default().into_config(cm);
        compress.module = false;
        compress.top_level = Some(TopLevelOptions { functions: true });
        compress.keep_fnames = true;
        compress.keep_classnames = true;

        Config {
            minify: MinifyOptions {
                compress: Some(compress),
                mangle: Some(MangleOptions {
                    top_level: Some(true),
                    keep_fn_names: true,
                    keep_class_names: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }
}

impl Config {
    /// Lo que corre `minifySync(src, { compress: {}, mangle: {} })` de @swc/core:
    /// la compresion por el mismo camino (`into_config`) sin tocar nada, y
    /// `MangleOptions` por defecto (sin `top_level`, sin `keep_*`). Una
    /// divergencia aca es de uso comun por construccion (policy de AGENTS.md).
    ///
    /// Sin `keep_*`, `console.log(f)` cambia de nombre: lo absorbe
    /// oracle::normalize_names. Un `f.name` impreso como string no: es
    /// divergencia esperada de esta config.
    pub fn core_defaults() -> Self {
        let cm: Lrc<SourceMap> = Default::default();
        Config {
            minify: MinifyOptions {
                compress: Some(TerserCompressorOptions::default().into_config(cm)),
                mangle: Some(MangleOptions::default()),
                ..Default::default()
            },
        }
    }

    /// core_defaults con los nombres preservados (keep_* en compress y mangle).
    /// No es una celda: sirve para preguntar si una divergencia de core_defaults
    /// desaparece cuando los nombres no cambian (main::names_only).
    pub fn core_defaults_keep_names() -> Self {
        let mut c = Config::core_defaults();
        if let Some(compress) = c.minify.compress.as_mut() {
            compress.keep_fnames = true;
            compress.keep_classnames = true;
        }
        if let Some(mangle) = c.minify.mangle.as_mut() {
            mangle.keep_fn_names = true;
            mangle.keep_class_names = true;
        }
        c
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
    /// `esm`: como modulo si parsea (carril B pendiente); solo se cuenta.
    ParseError { esm: bool },
    CodegenError,
    /// `optimize` (o algo en la cadena) entro en panico. Es un hallazgo.
    Panic(String),
}

pub fn prepare(src: &str, cfg: &Config) -> Outcome {
    // Globals nuevo por llamada: Mark::fresh() hace push sobre un Vec que vive
    // adentro y nunca se vacia (delta v1.2 §2.2).
    let result = catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || prepare_inner(src, cfg))
    }));
    match result {
        Ok(outcome) => outcome,
        Err(payload) => Outcome::Panic(panic_message(&payload)),
    }
}

fn prepare_inner(src: &str, cfg: &Config) -> Outcome {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());

    let mut recovered = Vec::new();
    let script = match parse_file_as_script(
        &fm,
        Syntax::default(),
        EsVersion::latest(),
        None,
        &mut recovered,
    ) {
        Ok(s) if recovered.is_empty() => s,
        _ => return Outcome::ParseError { esm: parses_as_module(&fm) },
    };

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();

    // paren_remover va ANTES del resolver, como en @swc/core. Sin el, los nodos
    // Paren del parser llegan al optimizador y algunas reglas no los ven: con
    // `delete (1 ? undefined : x)` el conditional se simplificaba adentro del
    // delete y quedaba `delete undefined` (los cuatro casos delete_* del corpus).
    let program = Program::Script(script)
        .apply(paren_remover(None))
        .apply(resolver(unresolved_mark, top_level_mark, false));
    let static_flags = crate::checks::static_flags(&program);

    let original = program.clone().apply(fixer(None));
    let dead_original = crate::checks::dead_writes(&original);
    let original = match emit(cm.clone(), &original) {
        Ok(s) => s,
        Err(_) => return Outcome::CodegenError,
    };

    let minified = optimize(
        program,
        cm.clone(),
        None,
        None,
        &cfg.minify,
        &ExtraOptions {
            unresolved_mark,
            top_level_mark,
            mangle_name_cache: None,
        },
    )
    .apply(fixer(None));
    let dead_minified = crate::checks::dead_writes(&minified);
    let minified = match emit(cm, &minified) {
        Ok(s) => s,
        Err(_) => return Outcome::CodegenError,
    };

    Outcome::Ok {
        original,
        minified,
        static_flags,
        dead_writes: (dead_original, dead_minified),
    }
}

/// Solo para el desglose de descartes: distingue ESM de sintaxis rechazada.
fn parses_as_module(fm: &SourceFile) -> bool {
    let mut recovered = Vec::new();
    parse_file_as_module(fm, Syntax::default(), EsVersion::latest(), None, &mut recovered).is_ok()
        && recovered.is_empty()
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
pub fn instrument(src: &str) -> Option<String> {
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
    Some(format!("{src}\n;\n__difuzzObserve({});\n", args.join(", ")))
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

/// Un solo printer, una sola config, para los dos lados.
fn emit(cm: Lrc<SourceMap>, program: &Program) -> Result<String, String> {
    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default().with_minify(true),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut buf, None),
        };
        emitter.emit_program(program).map_err(|e| e.to_string())?;
    }
    String::from_utf8(buf).map_err(|e| e.to_string())
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
    use super::{Config, Forma, Preset};

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
        let m = Config::core_defaults().minify.mangle.unwrap();
        assert_ne!(m.top_level, Some(true));
        assert!(!m.keep_fn_names && !m.keep_class_names);
    }
}
