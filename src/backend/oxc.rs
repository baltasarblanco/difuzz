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
//!   a la semantica (`SemanticBuilder::with_check_syntax_error`), que aca no
//!   corre; esos casos los descarta node (`a_invalid`).
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
use crate::minify::{Outcome, Preset};

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
    pub fn new(preset: Preset, source_type: SourceType) -> Self {
        match preset {
            Preset::CoreDefaults => Oxc {
                options: MinifierOptions::default(),
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
                compress: Some(CompressOptions {
                    keep_names: CompressOptionsKeepNames::all_true(),
                    ..CompressOptions::default()
                }),
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
        let (static_flags, dead_writes) = super::swc::checks_on_text(src, &original, &minified);
        Outcome::Ok {
            original,
            minified,
            static_flags,
            dead_writes,
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
        // Carril B: un modulo parsea. (`with` en modulo NO es error de parse en
        // oxc: lo deja a la semantica, que aca no corre; lo descarta node.)
        let m = Oxc::new(Preset::CoreDefaults, SourceType::mjs());
        assert!(matches!(m.prepare("export const a = 1;"), Outcome::Ok { .. }));
        assert!(matches!(
            m.prepare("var = ;"),
            Outcome::ParseError { esm: false, script: false }
        ));
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
}
