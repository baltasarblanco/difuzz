//! Backend SWC: la cadena parse → resolver → optimize → fixer → codegen.
//!
//! Una divergencia solo puede venir de `optimize` (mas resolver/fixer, que
//! corren en los dos lados). Es el codigo que antes vivia en minify.rs, sin
//! cambios de comportamiento.

use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, Mark, SourceFile, SourceMap, GLOBALS};
use swc_ecma_ast::{EsVersion, Program};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_minifier::{
    optimize,
    option::{
        terser::TerserCompressorOptions, ExtraOptions, MangleOptions, MinifyOptions,
        TopLevelOptions,
    },
};
use swc_ecma_parser::{parse_file_as_module, parse_file_as_script, EsSyntax, Syntax};
use swc_ecma_transforms_base::{
    fixer::{fixer, paren_remover},
    resolver,
};

use super::{Backend, Backlog, Politica};
use crate::checks::{Analisis, Parseo};
use crate::minify::{Outcome, SourceKind};

/// 7a: lo que difuzz sabe de SWC (antes en known.rs). Sus asunciones son las
/// de AGENTS.md del minifier; su backlog, tests/TODO.txt y tests/postponed.txt
/// del corpus. Sin re-chequeos de flags estaticos ni de sloppy.
pub const POLITICA: Politica = Politica {
    abreviatura: "swc",
    razones: FLAG_REASONS,
    rutas: ASSUMPTIONS,
    backlog: Some(Backlog {
        listas: &["TODO.txt", "postponed.txt"],
        prefijo: "terser/compress/",
        razon: "backlog de SWC (tests/TODO.txt o tests/postponed.txt)",
    }),
    // t-principio: los dos re-chequeos nuevos que no dependen del sensor (simetricos
    // con oxc). `proxy`, `tdz`, `callee`, `redefine_global`, el backlog y la ruta de
    // 2719 siguen yendo a known/ sin re-chequeo (swc es archivo).
    rechequeos: &[super::Rechequeo::Builtins, super::Rechequeo::Coercion],
    solo_sloppy: None,
    chequeos_propios: true,
    // Los de oxc (M13) no son asunciones de SWC: no se calculan.
    flags_estaticos: None,
};

/// (flag, asuncion que viola). Flags de preload.js y de checks.rs.
const FLAG_REASONS: &[(&str, &str)] = &[
    ("fn_tostring", "no depender de Function.prototype.toString()"),
    ("stack", "no depender de Error.prototype.stack"),
    ("proxy", "acceso a propiedades sin efectos (Proxy)"),
    ("builtins", "built-ins no pisados"),
    ("tdz", "TDZ no se preserva"),
    ("callee", "sin arguments.callee/.caller ni Function.prototype.caller"),
    ("redefine_global", "undefined/NaN/Infinity no se redefinen"),
    ("coercion", "toString/valueOf (helpers de coercion) sin efectos"),
    ("nombres", "core_defaults no preserva nombres de funciones/clases (keep_* apagado)"),
    // M16 (main.rs): no es asuncion del minifier, es el motor.
    ("profundidad", "difuzz: limite de pila del motor (V8), no del programa"),
];

/// (prefijo de ruta relativa al corpus, asuncion que el caso viola). Solo lo que
/// el sensor no puede ver: 2719 arma `"call" + "er"` en runtime, y V8 le da a
/// cada funcion sloppy su propio `caller` no configurable (no hay donde engancharse).
const ASSUMPTIONS: &[(&str, &str)] = &[
    (
        "terser/compress/issue_2719/",
        "asuncion: no se usa Function.prototype.caller",
    ),
];

pub struct Swc {
    pub minify: MinifyOptions,
    /// Carril: script (`.cjs`, sloppy) o modulo ES. Ver `with_kind`.
    pub kind: SourceKind,
}

impl Swc {
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
    /// - `compress.top_level = Some(..)`: sin esto `top_level()` da false, el
    ///   DCE top-level no corre y swc#12029 y swc#11963 quedan
    ///   invisibles.
    /// - `mangle.eval` queda en su default `false`: en el codigo es
    ///   `ignore_eval`, un opt-in que asume que ningun eval referencia
    ///   nombres locales. Prenderlo fabrica divergencias esperadas.
    /// - `mangle.props` queda en `None`: el corpus imprime objetos
    ///   libremente y el mangling de propiedades es observable via reflexion.
    ///   Prenderlo pide un corpus cuya salida controle el generador.
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

        Swc {
            kind: SourceKind::Script,
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
        Swc {
            kind: SourceKind::Script,
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
        let mut c = Swc::core_defaults();
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

impl Swc {
    /// El tipo de fuente que se parsea y las opciones que cambian con el.
    ///
    /// En modulo, lo que hace `@swc/core` en `minify` con un programa que parsea
    /// como modulo (crates/swc/src/lib.rs, `opts.toplevel == Some(true) ||
    /// program.is_module()`): `compress.top_level` y `mangle.top_level` se
    /// prenden si vienen en `None`. `compress.module` NO lo toca @swc/core (es
    /// `TerserCompressorOptions::module`, default `false`): por eso core_defaults
    /// lo deja en `false`, para seguir siendo lo que corre el producto.
    /// `estricto` (carril_a) describe el programa con precision y lo prende: un
    /// modulo ES siempre es estricto (`compress.module` → `in_strict` en
    /// optimize/pure, y `top_level()` true). En script no cambia nada.
    pub fn with_kind(mut self, kind: SourceKind, estricto: bool) -> Self {
        self.kind = kind;
        if kind == SourceKind::Module {
            if let Some(compress) = self.minify.compress.as_mut() {
                if compress.top_level.is_none() {
                    compress.top_level = Some(TopLevelOptions { functions: true });
                }
                if estricto {
                    compress.module = true;
                }
            }
            if let Some(mangle) = self.minify.mangle.as_mut() {
                if mangle.top_level.is_none() {
                    mangle.top_level = Some(true);
                }
            }
        }
        self
    }
}

impl Backend for Swc {
    fn prepare(&self, src: &str) -> Outcome {
        // Globals nuevo por llamada: Mark::fresh() hace push sobre un Vec que vive
        // adentro y nunca se vacia (delta v1.2 §2.2).
        GLOBALS.set(&Globals::new(), || prepare_inner(src, self))
    }
}

fn prepare_inner(src: &str, cfg: &Swc) -> Outcome {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());

    let Some(parsed) = parse_program(&fm, cfg.kind) else {
        return match cfg.kind {
            SourceKind::Script => Outcome::ParseError { esm: parses_as_module(&fm), script: false },
            SourceKind::Module => Outcome::ParseError {
                esm: false,
                script: parse_program(&fm, SourceKind::Script).is_some(),
            },
        };
    };

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();

    // paren_remover va ANTES del resolver, como en @swc/core. Sin el, los nodos
    // Paren del parser llegan al optimizador y algunas reglas no los ven: con
    // `delete (1 ? undefined : x)` el conditional se simplificaba adentro del
    // delete y quedaba `delete undefined` (los cuatro casos delete_* del corpus).
    let program = parsed
        .apply(paren_remover(None))
        .apply(resolver(unresolved_mark, top_level_mark, false));
    // R1-H12: los chequeos son analisis de difuzz, no del minifier: un panic
    // suyo no es hallazgo (analisis::atrapar; la marca la saca Analizado).
    let mut fallo: Option<String> = None;
    let static_flags = crate::analisis::atrapar(|| crate::checks::static_flags(&program)).unwrap_or_else(|e| {
        fallo.get_or_insert(e);
        Vec::new()
    });

    let original = program.clone().apply(fixer(None));
    let dead_original = crate::analisis::atrapar(|| crate::checks::dead_writes(&original)).unwrap_or_else(|e| {
        fallo.get_or_insert(e);
        0
    });
    let original = match emit(cm.clone(), &original) {
        Ok(s) => s,
        Err(_) => return Outcome::CodegenError,
    };

    // R1-H21: `optimize` no es la identidad aunque `compress` y `mangle` vengan en
    // `None`. La auditoria A1 de una revision del 07-oct (MEDIDO por el revisor;
    // hoy la repite `a1_mismo_printer` en tests/muestra/auditoria.rs sobre la
    // muestra fija) paso los 3119 fixtures con los dos en `None`:
    // en carril B, 3051 de 3053 salen iguales al original y 2 no, porque igual
    // reescribe los `export {...}`: `terser/compress/export/issue_2126` fusiona
    // `export{bar as qux};export{dog}` en `export{bar as qux,dog}` y
    // `terser/compress/export/redirection` los reordena. Consecuencia: con SWC
    // "mismo printer" (original y minificado salen del mismo `emit`) no garantiza
    // que toda diferencia de texto venga de la compresion o del mangle; en
    // codigo con exports parte puede ser de `optimize` mismo. Carril A y oxc: 0
    // diferencias en esa auditoria. No se corrige aca: el original es el programa
    // tal cual, sin pasar por `optimize`.
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
    let dead_minified = crate::analisis::atrapar(|| crate::checks::dead_writes(&minified)).unwrap_or_else(|e| {
        fallo.get_or_insert(e);
        0
    });
    // b2-int (R1-H13): con SWC el analisis es sobre su propio AST (limpio),
    // salvo que los chequeos fallen: entonces los tres textos quedan perdidos.
    let (static_flags, dead_original, dead_minified, analisis) = match fallo {
        Some(e) => {
            crate::analisis::fallo_en_backend(e);
            (vec![crate::analisis::FALLO_ANALISIS], 0, 0, Analisis::PERDIDO)
        }
        None => (static_flags, dead_original, dead_minified, Analisis::default()),
    };
    let minified = match emit(cm, &minified) {
        Ok(s) => s,
        Err(_) => return Outcome::CodegenError,
    };

    Outcome::Ok {
        original,
        minified,
        static_flags,
        dead_writes: (dead_original, dead_minified),
        analisis,
    }
}

/// Parse sin errores recuperados, como script o como modulo segun el carril.
/// `None` si el parser rechaza el fuente o lo acepta recuperando errores.
pub(crate) fn parse_program(fm: &SourceFile, kind: SourceKind) -> Option<Program> {
    let mut recovered = Vec::new();
    let program = match kind {
        // M19: el carril A es CommonJS: `return` top-level es valido (node lo
        // envuelve en una funcion) y oxc (`cjs()`) lo acepta. `new.target`
        // top-level tambien es valido en CJS, pero SWC no tiene opcion para eso
        // (NO SE si existe): sigue siendo descarte `parse`.
        SourceKind::Script => parse_file_as_script(
            fm,
            Syntax::Es(EsSyntax { allow_return_outside_function: true, ..Default::default() }),
            EsVersion::latest(),
            None,
            &mut recovered,
        )
        .ok()
        .map(Program::Script),
        SourceKind::Module => {
            parse_file_as_module(fm, Syntax::default(), EsVersion::latest(), None, &mut recovered)
                .ok()
                .map(Program::Module)
        }
    };
    program.filter(|_| recovered.is_empty())
}

/// Nota transversal 2 (M12, M14, M15): parse con SWC de un texto suelto, segun
/// el carril (script o modulo), sin errores recuperados. Un solo helper para el
/// epilogo observador, los chequeos sobre texto de otro backend y las mutaciones.
pub fn parse_for(src: &str, kind: SourceKind) -> Option<Program> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    parse_program(&fm, kind)
}

/// M13 (re-chequeo de `extends`): el mismo programa con cada heredero literal
/// (`class extends 1`, regex, array, objeto, template) cambiado por `null`, que
/// no lanza al definir la clase. `None` si no parsea o no hay nada que cambiar.
pub fn neutralize_extends(src: &str, kind: SourceKind) -> Option<String> {
    use swc_ecma_ast::{Class, Expr, Lit, Null};
    use swc_ecma_visit::{VisitMut, VisitMutWith};
    struct N(bool);
    impl VisitMut for N {
        fn visit_mut_class(&mut self, c: &mut Class) {
            if let Some(sup) = c.super_class.as_mut() {
                let mut e: &Expr = sup;
                while let Expr::Paren(p) = e {
                    e = &p.expr;
                }
                let lit = matches!(e, Expr::Lit(l) if !matches!(l, Lit::Null(_)))
                    || matches!(e, Expr::Array(_) | Expr::Object(_) | Expr::Tpl(_));
                if lit {
                    **sup = Expr::Lit(Lit::Null(Null { span: Default::default() }));
                    self.0 = true;
                }
            }
            c.visit_mut_children_with(self);
        }
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = parse_program(&fm, kind)?;
            let mut n = N(false);
            program.visit_mut_with(&mut n);
            if !n.0 {
                return None;
            }
            emit(cm, &program.apply(fixer(None))).ok()
        })
    }))
    .ok()
    .flatten()
}

/// Solo para el desglose de descartes: distingue ESM de sintaxis rechazada.
fn parses_as_module(fm: &SourceFile) -> bool {
    let mut recovered = Vec::new();
    parse_file_as_module(fm, Syntax::default(), EsVersion::latest(), None, &mut recovered).is_ok()
        && recovered.is_empty()
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

/// Los chequeos de checks.rs para un backend que no es SWC: el parser de SWC
/// solo analiza, no minifica. `src` da los flags estaticos; `original` y
/// `minified` (los textos que imprimio el otro backend) dan dead_writes. Un
/// texto que SWC no acepta (o un panic de SWC) cuenta como sin flags y 0
/// escrituras: un panic aca no es un hallazgo del backend medido.
///
/// M14: se parsea segun el carril (`kind`). Antes siempre como script: en el
/// carril B un modulo con import/export no parseaba y quedaba sin flags ni lint.
///
/// R1-H13: ya no en silencio. Cada texto se parsea con `parse_para_analisis`
/// (acepta errores recuperados) y el estado de los tres vuelve en `Analisis`:
/// main lo cuenta en las stats y lo anota en meta.txt. Un panic de SWC deja
/// todo `Perdido`.
///
/// 09-oct: `minifier` es el backend medido; ademas de los flags comunes se
/// calculan los que declara su politica (`Politica::flags_estaticos`).
pub fn checks_on_text(
    src: &str,
    original: &str,
    minified: &str,
    kind: SourceKind,
    minifier: super::Minifier,
) -> (Vec<&'static str>, (usize, usize), Analisis) {
    let perdido = Analisis { fuente: Parseo::Perdido, original: Parseo::Perdido, minificado: Parseo::Perdido };
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let (p, fuente) = parse_para_analisis(src, kind);
            let propios = minifier.politica().flags_estaticos;
            let static_flags = p.map_or_else(Vec::new, |p| {
                let mut f = crate::checks::static_flags(&p);
                f.extend(propios.map_or_else(Vec::new, |g| g(&p)));
                f
            });
            let dead = |text: &str| {
                let (p, estado) = parse_para_analisis(text, kind);
                (p.map_or(0, |p| crate::checks::dead_writes(&p)), estado)
            };
            let ((d0, original), (d1, minificado)) = (dead(original), dead(minified));
            (static_flags, (d0, d1), Analisis { fuente, original, minificado })
        })
    }))
    .unwrap_or((Vec::new(), (0, 0), perdido))
}

/// R1-H13: parse para ANALIZAR (flags, lint, bindings del epilogo), no para
/// minificar. Primero como `parse_for` (segun el carril, sin errores). Si no:
/// el mismo carril aceptando errores recuperados (el parser de SWC marca como
/// recuperables `new.target` top-level, una funcion de Anexo B como cuerpo de
/// un `if`, `delete x` en estricto; el AST esta entero), y por ultimo el otro
/// carril. MEDIDO en el corpus de SWC con oxc: eso recupera todos los textos
/// que el revisor 1 encontro perdidos salvo un minificado (fixture 12214, error
/// fatal en los dos carriles).
pub fn parse_para_analisis(src: &str, kind: SourceKind) -> (Option<Program>, Parseo) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    if let Some(p) = parse_program(&fm, kind) {
        return (Some(p), Parseo::Limpio);
    }
    let otro = match kind {
        SourceKind::Script => SourceKind::Module,
        SourceKind::Module => SourceKind::Script,
    };
    for k in [kind, otro] {
        if let Some(p) = parse_tolerante(&fm, k) {
            return (Some(p), Parseo::Recuperado);
        }
    }
    (None, Parseo::Perdido)
}

/// Como `parse_program`, pero acepta errores recuperados.
fn parse_tolerante(fm: &SourceFile, kind: SourceKind) -> Option<Program> {
    let mut recovered = Vec::new();
    match kind {
        SourceKind::Script => parse_file_as_script(
            fm,
            Syntax::Es(EsSyntax { allow_return_outside_function: true, ..Default::default() }),
            EsVersion::latest(),
            None,
            &mut recovered,
        )
        .ok()
        .map(Program::Script),
        SourceKind::Module => parse_file_as_module(fm, Syntax::default(), EsVersion::latest(), None, &mut recovered)
            .ok()
            .map(Program::Module),
    }
}

#[cfg(test)]
mod tests {
    use super::Swc;
    use crate::backend::Backend;
    use crate::minify::{Outcome, SourceKind};

    #[test]
    fn carril_b_parsea_como_modulo() {
        let esm = "import { x } from './y.mjs';\nexport const a = x + 1;\nexport default function f() { return a; }\n";
        let cfg = Swc::carril_a().with_kind(SourceKind::Module, true);
        let Outcome::Ok { original, minified, .. } = cfg.prepare(esm) else {
            panic!("un modulo tiene que parsear en carril B");
        };
        assert!(original.contains("import") && original.contains("export"));
        // Los exports son usados por definicion: el minifier no los puede borrar.
        assert!(minified.contains("export"));
        // El mismo fuente en carril A es el descarte de siempre (esm).
        assert!(matches!(
            Swc::carril_a().prepare(esm),
            Outcome::ParseError { esm: true, script: false }
        ));
        // El espejo: `with` es solo de script (modulo = estricto).
        assert!(matches!(
            cfg.prepare("with (o) { x; }"),
            Outcome::ParseError { esm: false, script: true }
        ));
        assert!(matches!(
            cfg.prepare("let = ;"),
            Outcome::ParseError { esm: false, script: false }
        ));
        // M19: `return` top-level es CommonJS valido en el carril A, no en el B.
        assert!(matches!(Swc::carril_a().prepare("console.log(1);\nreturn;\n"), Outcome::Ok { .. }));
        assert!(matches!(cfg.prepare("console.log(1);\nreturn;\n"), Outcome::ParseError { .. }));
    }

    /// R1-H13: lo que SWC solo parsea con errores recuperados (o con el otro
    /// carril) se analiza igual y se avisa; lo que no parsea queda `Perdido`.
    #[test]
    fn analisis_no_es_silencioso() {
        use super::checks_on_text;
        use crate::checks::Parseo::{Limpio, Perdido, Recuperado};
        // `with` (abajo) es un flag estatico de oxc: solo lo calcula su politica.
        const OXC: crate::backend::Minifier = crate::backend::Minifier::Oxc;
        // `new.target` top-level (CJS valido, SWC lo marca como recuperado).
        let (_, _, a) = checks_on_text("new.target;\n", "new.target;\n", "x = {", SourceKind::Script, OXC);
        assert_eq!((a.fuente, a.original, a.minificado), (Recuperado, Recuperado, Perdido));
        // Anexo B como cuerpo de un `if`: los flags se calculan igual.
        let (flags, _, a) = checks_on_text("if (x) function f() { return arguments.callee; }\n", "1;", "1;", SourceKind::Script, OXC);
        assert_eq!((flags, a.fuente, a.original), (vec!["callee"], Recuperado, Limpio));
        // `delete x` en un modulo (estricto): el lint cuenta igual.
        let (_, dead, a) = checks_on_text("1;", "1;", "delete x; [1][0] = 2;", SourceKind::Module, OXC);
        assert_eq!((dead, a.minificado), ((0, 1), Recuperado));
        // `with` en un modulo no parsea como modulo: se analiza como script.
        let (flags, _, a) = checks_on_text("with (o) { x; }", "1;", "1;", SourceKind::Module, OXC);
        assert_eq!((flags, a.fuente), (vec!["with"], Recuperado));
        let (_, _, a) = checks_on_text("1;", "1;", "1;", SourceKind::Module, OXC);
        assert!(a.limpio());
    }

    /// M13: los herederos literales pasan a `null`; los demas no se tocan.
    #[test]
    fn neutraliza_extends_literales() {
        use super::neutralize_extends;
        let out = neutralize_extends("class A extends 1 {}\nvar B = class extends ([2]) {};\nclass C extends A {}\n", SourceKind::Script).unwrap();
        assert_eq!(out.matches("extends null").count(), 2, "{out}");
        assert!(out.contains("extends A"), "{out}");
        assert!(neutralize_extends("class C extends D {}\n", SourceKind::Script).is_none());
    }

    #[test]
    fn opciones_de_modulo() {
        // carril_a en modulo: compress.module (estricto) ademas de top_level.
        let c = Swc::carril_a().with_kind(SourceKind::Module, true).minify;
        assert!(c.compress.as_ref().unwrap().module);
        assert_eq!(c.mangle.as_ref().unwrap().top_level, Some(true));
        // En script no cambia nada respecto de carril_a().
        assert!(!Swc::carril_a().with_kind(SourceKind::Script, true).minify.compress.unwrap().module);
        // core_defaults en modulo: lo de @swc/core (top_level si, compress.module no).
        let d = Swc::core_defaults().with_kind(SourceKind::Module, false).minify;
        let dc = d.compress.unwrap();
        assert!(!dc.module);
        assert!(dc.top_level.is_some_and(|t| t.functions));
        assert_eq!(d.mangle.unwrap().top_level, Some(true));
        let s = Swc::core_defaults().with_kind(SourceKind::Script, false).minify;
        assert_ne!(s.mangle.unwrap().top_level, Some(true));
    }
}
