//! Backend oxc: parse (oxc_parser) → codegen del original → minify → codegen.
//!
//! Se parsea una sola vez. El original es ese programa impreso con
//! `oxc_codegen` en modo minify; despues el minifier lo transforma en el lugar
//! y se imprime con el mismo codegen, como lo hace el producto (napi/minify:
//! `with_scoping` + `with_private_member_mappings`). Asi una divergencia solo
//! puede venir de `Minifier::minify` (compress + mangle).
//!
//! Lo que el backend SWC tenia y oxc no:
//! - No hay `top_level` en compress: el DCE top-level depende del tipo de
//!   fuente (corre para modulo y CommonJS, no para script).
//! - `ParseError` sin errores recuperados: oxc deja algunos errores tempranos
//!   a la semantica (`SemanticBuilder::with_check_syntax_error`). Desde M19
//!   corre despues del parse y sus errores tambien son `ParseError`.
//! - `CodegenError` no existe: `Codegen::build` no falla.

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_minifier::{
    CompressOptions, CompressOptionsKeepNames, MangleOptions, MangleOptionsKeepNames, Minifier,
    MinifierOptions,
};
use oxc_parser::Parser;
pub use oxc_span::SourceType;

use super::Backend;
use crate::minify::{Outcome, Preset, SourceKind};

pub struct Oxc {
    options: MinifierOptions,
    /// `SourceType::cjs()` para el carril actual (node corre `.cjs`, sloppy).
    source_type: SourceType,
}

impl Oxc {
    /// - `core_defaults`: lo que corre `minifySync(name, src)` de `oxc-minify`
    ///   sin opciones: `MinifierOptions::default()`, o sea
    ///   `CompressOptions::default()` (= `smallest()`, sin keep_names) y
    ///   `MangleOptions::default()` (`top_level` sale del tipo de fuente: true
    ///   para modulo y CommonJS).
    /// - `carril_a`: lo mismo con los nombres preservados en compress y en
    ///   mangle, y `top_level` explicito. Es la intencion del carril A de SWC
    ///   (DCE top-level + keep_*). En oxc coincide con core_defaults_keep_names.
    ///
    /// M35 (decision del usuario, opcion a): en los tres presets
    /// `treeshake.annotations = false`. `core_defaults` se aparta de `minifySync`
    /// SOLO en eso: oxc honra `/*@__PURE__*/` y `/*#__NO_SIDE_EFFECTS__*/` por
    /// contrato y borra llamadas anotadas con efectos (7 falsos positivos por
    /// celda); SWC no las ve (emite y parsea sin comentarios). Se pierde la
    /// cobertura del camino de anotaciones.
    pub fn new(preset: Preset, source_type: SourceType) -> Self {
        match preset {
            Preset::CoreDefaults => Oxc {
                options: MinifierOptions {
                    compress: Some(sin_anotaciones(CompressOptions::default())),
                    ..MinifierOptions::default()
                },
                source_type,
            },
            Preset::CarrilA => {
                let mut o = Oxc::core_defaults_keep_names(source_type);
                if let Some(mangle) = o.options.mangle.as_mut() {
                    mangle.top_level = Some(true);
                }
                o
            }
        }
    }

    /// core_defaults con `keep_names` en compress y en mangle (main::names_only).
    pub fn core_defaults_keep_names(source_type: SourceType) -> Self {
        Oxc {
            options: MinifierOptions {
                compress: Some(sin_anotaciones(CompressOptions {
                    keep_names: CompressOptionsKeepNames::all_true(),
                    ..CompressOptions::default()
                })),
                mangle: Some(MangleOptions {
                    keep_names: MangleOptionsKeepNames::all_true(),
                    ..MangleOptions::default()
                }),
                mangle_properties: None,
            },
            source_type,
        }
    }
}

/// M35: no honrar las anotaciones PURE / NO_SIDE_EFFECTS (ver `Oxc::new`).
fn sin_anotaciones(mut c: CompressOptions) -> CompressOptions {
    c.treeshake.annotations = false;
    c
}

impl Backend for Oxc {
    fn prepare(&self, src: &str) -> Outcome {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, src, self.source_type).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() {
            // El espejo del otro carril, solo para el desglose de descartes.
            return if self.source_type.is_module() {
                Outcome::ParseError { esm: false, script: parses_as(src, SourceType::cjs()) }
            } else {
                Outcome::ParseError { esm: parses_as(src, SourceType::mjs()), script: false }
            };
        }
        let mut program = parsed.program;
        // M19: los errores tempranos que el parser de oxc deja a la semantica
        // (octal legacy en estricto, etc.). Sin esto, su codegen "arreglaba" un
        // modulo invalido y node corria el original impreso como si fuera valido.
        let semantic_errors = !oxc_semantic::SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program)
            .diagnostics
            .is_empty();
        if semantic_errors {
            return if self.source_type.is_module() {
                Outcome::ParseError { esm: false, script: parses_as(src, SourceType::cjs()) }
            } else {
                Outcome::ParseError { esm: parses_as(src, SourceType::mjs()), script: false }
            };
        }

        let original = Codegen::new()
            .with_options(CodegenOptions::minify())
            .build(&program)
            .code;

        let ret = Minifier::new(self.options.clone()).minify(&allocator, &mut program);
        let minified = Codegen::new()
            .with_options(CodegenOptions::minify())
            .with_scoping(ret.scoping)
            .with_private_member_mappings(ret.class_private_mappings)
            .build(&program)
            .code;

        // checks.rs es analisis sobre el AST de SWC: se reparsean los textos.
        let kind = if self.source_type.is_module() { SourceKind::Module } else { SourceKind::Script };
        // A6/R1-H12: en el hijo minificador no (los pide analisis::Analizado al
        // hijo de analisis, que tambien trae el estado R1-H13 del analisis); en
        // proceso, un fallo no es del minifier y deja los tres textos perdidos.
        let (static_flags, dead_writes, analisis) = if crate::analisis::checks_afuera() {
            (Vec::new(), (0, 0), Default::default())
        } else {
            crate::analisis::atrapar(|| super::swc::checks_on_text(src, &original, &minified, kind)).unwrap_or_else(|e| {
                crate::analisis::fallo_en_backend(e);
                (vec![crate::analisis::FALLO_ANALISIS], (0, 0), crate::checks::Analisis::PERDIDO)
            })
        };
        Outcome::Ok {
            original,
            minified,
            static_flags,
            dead_writes,
            analisis,
        }
    }
}

/// Solo para el desglose de descartes: si el fuente parsea con el otro tipo.
fn parses_as(src: &str, source_type: SourceType) -> bool {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, source_type).parse();
    !parsed.fatal_error && parsed.diagnostics.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{Oxc, SourceType};
    use crate::backend::Backend;
    use crate::minify::{Outcome, Preset};

    fn prepare(preset: Preset, src: &str) -> (String, String, Vec<&'static str>, (usize, usize)) {
        match Oxc::new(preset, SourceType::cjs()).prepare(src) {
            Outcome::Ok {
                original,
                minified,
                static_flags,
                dead_writes,
                ..
            } => (original, minified, static_flags, dead_writes),
            _ => panic!("oxc no preparo {src:?}"),
        }
    }

    #[test]
    fn oxc_imprimir_el_original_es_idempotente() {
        let (original, _, _, _) =
            prepare(Preset::CoreDefaults, "var a = 1 + 2;\nconsole.log(a);\n");
        // Reimprimir lo impreso no cambia nada: el printer no reescribe semantica.
        let (again, _, _, _) = prepare(Preset::CoreDefaults, &original);
        assert_eq!(original, again);
        assert!(
            original.contains("1 + 2") || original.contains("1+2"),
            "{original}"
        );
    }

    #[test]
    fn oxc_core_defaults_minifica_algo_observable() {
        let src =
            "function unused() { return 1; }\nvar x = 1 + 2;\nif (true) { console.log(x); }\n";
        let (original, minified, _, _) = prepare(Preset::CoreDefaults, src);
        assert!(original.contains("unused"), "{original}");
        // CommonJS: DCE top-level y plegado de constantes.
        assert!(!minified.contains("unused"), "{minified}");
        assert!(
            minified.contains('3') && !minified.contains("1+2"),
            "{minified}"
        );
        assert!(minified.len() < original.len());
    }

    #[test]
    fn oxc_carril_a_preserva_nombres() {
        let src = "function named() {}\nconsole.log(named.name);\n";
        let (_, minified, _, _) = prepare(Preset::CarrilA, src);
        assert!(minified.contains("named"), "{minified}");
    }

    #[test]
    fn oxc_parse_error_y_esm() {
        let b = Oxc::new(Preset::CoreDefaults, SourceType::cjs());
        assert!(matches!(
            b.prepare("export const a = 1;"),
            Outcome::ParseError { esm: true, script: false }
        ));
        assert!(matches!(
            b.prepare("var = ;"),
            Outcome::ParseError { esm: false, script: false }
        ));
        // Carril B: un modulo parsea. M19: lo que el parser deja a la semantica
        // (`with`, octal legacy en estricto) ahora tambien es ParseError.
        let m = Oxc::new(Preset::CoreDefaults, SourceType::mjs());
        assert!(matches!(m.prepare("export const a = 1;"), Outcome::Ok { .. }));
        assert!(matches!(m.prepare("with (o) { x; }"), Outcome::ParseError { esm: false, script: true }));
        assert!(matches!(m.prepare("console.log(010);"), Outcome::ParseError { esm: false, script: true }));
        assert!(matches!(m.prepare("console.log(\"\\101\");"), Outcome::ParseError { .. }));
        assert!(matches!(b.prepare("console.log(010);"), Outcome::Ok { .. }));
        assert!(matches!(
            m.prepare("var = ;"),
            Outcome::ParseError { esm: false, script: false }
        ));
    }

    /// M35: una llamada anotada con efectos se conserva en los tres presets.
    #[test]
    fn oxc_no_honra_anotaciones_pure() {
        let src = "/*@__PURE__*/ console.log(\"efecto\");\n";
        for preset in [Preset::CoreDefaults, Preset::CarrilA] {
            let (_, minified, _, _) = prepare(preset, src);
            assert!(minified.contains("efecto"), "{preset:?}: {minified}");
        }
        let Outcome::Ok { minified, .. } = Oxc::core_defaults_keep_names(SourceType::cjs()).prepare(src) else {
            panic!("no preparo");
        };
        assert!(minified.contains("efecto"), "{minified}");
        // Control: con las opciones de minifySync (annotations = true) se borra.
        let npm = Oxc { options: oxc_minifier::MinifierOptions::default(), source_type: SourceType::cjs() };
        let Outcome::Ok { minified, .. } = npm.prepare(src) else {
            panic!("no preparo");
        };
        assert!(!minified.contains("efecto"), "{minified}");
    }

    #[test]
    fn oxc_chequeos_estaticos_con_swc() {
        let (_, _, flags, dead) = prepare(
            Preset::CoreDefaults,
            "var o = { valueOf() { return 1 } };\n[1, 2][0] = 3;\nconsole.log(+o);\n",
        );
        assert_eq!(flags, ["coercion"]);
        assert_eq!(dead.0, 1);
    }

    /// M14: en el carril B los chequeos parsean como modulo.
    #[test]
    fn oxc_chequeos_estaticos_en_modulo() {
        let m = Oxc::new(Preset::CoreDefaults, SourceType::mjs());
        let Outcome::Ok { static_flags, dead_writes, .. } =
            m.prepare("export const o = { valueOf() { return 1 } };\n[1, 2][0] = 3;\nconsole.log(+o);\n")
        else {
            panic!("un modulo tiene que preparar");
        };
        assert_eq!(static_flags, ["coercion"]);
        assert_eq!(dead_writes.0, 1);
    }
}
