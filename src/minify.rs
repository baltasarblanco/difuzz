//! MIN: la cadena parse → resolver → optimize → fixer → codegen.
//!
//! Devuelve DOS programas impresos con el mismo parser y el mismo printer:
//! el original sin optimizar y el minificado. Asi, una divergencia entre
//! ambos solo puede venir de `optimize` (mas resolver/fixer, que corren en
//! los dos lados). Comparar contra el texto fuente crudo mezclaria bugs de
//! parser/codegen con bugs del minifier.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_ecma_ast::{EsVersion, Program};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_minifier::{
    optimize,
    option::{CompressOptions, ExtraOptions, MangleOptions, MinifyOptions, TopLevelOptions},
};
use swc_ecma_parser::{parse_file_as_script, Syntax};
use swc_ecma_transforms_base::{fixer::fixer, resolver};

pub struct Config {
    pub name: &'static str,
    pub minify: MinifyOptions,
}

impl Config {
    /// Carril A: script + CommonJS (delta v1.2, decision #1).
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
    pub fn carril_a() -> Self {
        Config {
            name: "carril_a",
            minify: MinifyOptions {
                compress: Some(CompressOptions {
                    module: false,
                    top_level: Some(TopLevelOptions { functions: true }),
                    ..Default::default()
                }),
                mangle: Some(MangleOptions {
                    top_level: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }
}

pub enum Outcome {
    Ok { original: String, minified: String },
    /// El parser rechazo el fuente o lo acepto con errores recuperados.
    ParseError(String),
    CodegenError(String),
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
        Ok(s) => s,
        Err(e) => return Outcome::ParseError(format!("{e:?}")),
    };
    if !recovered.is_empty() {
        return Outcome::ParseError(format!("{} errores recuperados", recovered.len()));
    }

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();

    let program =
        Program::Script(script).apply(resolver(unresolved_mark, top_level_mark, false));

    let original = program.clone().apply(fixer(None));
    let original = match emit(cm.clone(), &original) {
        Ok(s) => s,
        Err(e) => return Outcome::CodegenError(format!("original: {e}")),
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
    let minified = match emit(cm, &minified) {
        Ok(s) => s,
        Err(e) => return Outcome::CodegenError(format!("minificado: {e}")),
    };

    Outcome::Ok { original, minified }
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
