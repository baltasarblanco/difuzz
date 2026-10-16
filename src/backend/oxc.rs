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

use super::{Backend, Politica, Rechequeo};
use crate::minify::{Outcome, Preset, SourceKind};

/// 7a: lo que difuzz sabe de oxc (antes en known.rs, main.rs y recheck.rs).
/// Sin backlog ni asunciones por ruta (las de terser son de SWC). Los flags
/// estaticos `extends`, `global_conocido` y `with` (M13) se re-chequean antes
/// de clasificar, y en el carril A una divergencia sin asuncion se prueba en
/// estricto (M17: oxc no soporta sloppy: lo que solo pasa en sloppy no es un bug para oxc).
pub const POLITICA: Politica = Politica {
    abreviatura: "oxc",
    razones: FLAG_REASONS_OXC,
    rutas: &[],
    backlog: None,
    // t-principio: `builtins` y `coercion` tambien (antes iban a known/ sin re-chequeo).
    rechequeos: &[
        Rechequeo::Extends,
        Rechequeo::GlobalConocido,
        Rechequeo::With,
        Rechequeo::Violaciones,
        Rechequeo::Builtins,
        Rechequeo::Coercion,
        // 8.3: el nombre local en un mensaje de V8 (`Oxc::sin_mangle`).
        Rechequeo::MensajeNombre,
    ],
    solo_sloppy: Some("oxc: solo sloppy (no soportado); con \"use strict\" la divergencia desaparece"),
    chequeos_propios: false,
    // M13: `with`, `global_conocido` y `extends` (los re-chequea `rechequeos`).
    flags_estaticos: Some(crate::checks::static_flags_oxc),
};

/// Las asunciones de oxc, contra crates/oxc_minifier/docs/ASSUMPTIONS.md (tag
/// crates_v0.153.0). Asunciones de oxc SIN flag todavia (sus divergencias caen
/// en div/): `document.all`, longitud maxima de Array/String/TypedArray,
/// `var` de un eval directo usada afuera, `arguments` global, y escrituras de propiedades sobre
/// bindings no usados (setters en prototipos: solo cubierto si el preload ve
/// `builtins`). M13 agrego `with`, `global_conocido` y `extends`; b4-a,
/// `tdz_atrapada`, `coercion_symbol` y `extends_atrapado`.
/// Flags de SWC que oxc NO declara como asuncion y por eso no van aca:
/// `proxy` (property_read_side_effects es All por defecto), `callee`,
/// `redefine_global`.
const FLAG_REASONS_OXC: &[(&str, &str)] = &[
    ("fn_tostring", "oxc: Function.prototype.toString is not relied on"),
    ("builtins", "oxc: No Monkey-Patching Built-ins"),
    // I2 (revision de los bloques 4-6): la TDZ no atrapada tambien pasa por el re-chequeo de
    // violaciones (recheck::FLAGS_VIOLACION); solo llega aca si dio Explica.
    ("tdz", "oxc: No TDZ Violation"),
    ("coercion", "oxc: Coercion Methods Are Pure"),
    ("nombres", "oxc: No Reliance on Function.prototype.name (keep_names apagado en core_defaults)"),
    // M13 (checks.rs y preload.js).
    ("with", "oxc: No `with` Statement"),
    ("global_conocido", "oxc: Known globals exist and are side-effect-free to access (no existe en node)"),
    ("extends", "oxc: No side effects from extending a class"),
    // b4-a (R1-H04 / C3, preload.js): violaciones que el programa atrapa (y la
    // coercion de Symbol, atrapada o no). Solo llegan aca si el re-chequeo de
    // violaciones (Rechequeo::Violaciones) dio Explica; si no, se renombran.
    ("tdz_atrapada", "oxc: No TDZ Violation (atrapada)"),
    ("coercion_symbol", "oxc: Coercion to a primitive does not throw for non-Symbol arguments"),
    ("extends_atrapado", "oxc: No side effects from extending a class (atrapado)"),
    // M16 (main.rs): no es asuncion del minifier, es el motor.
    ("profundidad", "difuzz: limite de pila del motor (V8), no del programa"),
    // No es asuncion documentada de oxc: el contenido de .stack (posiciones,
    // nombres) cambia con cualquier minificacion. Decision de difuzz, VERIFICAR.
    ("stack", "difuzz: el contenido de Error.prototype.stack cambia al minificar (oxc no lo documenta)"),
    // 8.3 (recheck::mensaje_nombre_matters): solo llega aca si el re-chequeo dio Explica (B es
    // un renombre alfa-equivalente del minificado sin mangle, y ese da lo mismo que A). No es
    // asuncion documentada de oxc: V8 arma `X is not a function` con el texto de la llamada.
    ("mensaje_nombre", "difuzz: V8 escribe en el mensaje de error el nombre local de la expresion, que el mangle cambia (oxc no lo documenta)"),
];

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
    /// M35 (decision del 06-oct, opcion a): en los tres presets
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

    /// El preset con los nombres preservados (main::names_only), con el resto
    /// de sus opciones intactas. Solo para los presets que cambian nombres (hoy
    /// `core_defaults`); para los otros devuelve el preset tal cual.
    pub fn con_nombres(preset: Preset, source_type: SourceType) -> Self {
        match preset {
            Preset::CoreDefaults => Oxc::core_defaults_keep_names(source_type),
            Preset::CarrilA => Oxc::new(preset, source_type),
        }
    }

    /// 8.3 (`Rechequeo::MensajeNombre`): el preset con mangle apagado y compress igual. En
    /// oxc_minifier 0.153.0 compress corre antes que mangle y no depende de el
    /// (`Minifier::build`, lib.rs:159-191): el programa que sale es el de B antes del renombre.
    pub fn sin_mangle(preset: Preset, source_type: SourceType) -> Self {
        let mut o = Oxc::new(preset, source_type);
        o.options.mangle = None;
        o
    }

    /// 8.3 (H1): el mangle preserva el `.name` de funciones o de clases
    /// (`MangleOptionsKeepNames`, oxc_mangler-0.153.0/src/keep_names.rs:7-17).
    pub fn preserva_nombres(&self) -> bool {
        self.options.mangle.as_ref().is_some_and(|m| m.keep_names.function || m.keep_names.class)
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
            crate::analisis::atrapar(|| super::swc::checks_on_text(src, &original, &minified, kind, super::Minifier::Oxc)).unwrap_or_else(|e| {
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
/// R1-H06: con el mismo criterio que `prepare` (parser y despues los errores
/// tempranos de la semantica, M19). Antes solo el parser: un fuente que la
/// semantica rechaza en los dos tipos salia `esm=1` en A y `script=1` en B.
fn parses_as(src: &str, source_type: SourceType) -> bool {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src, source_type).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return false;
    }
    oxc_semantic::SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&parsed.program)
        .diagnostics
        .is_empty()
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

    /// R1-H06: el rotulo del otro carril usa la semantica, como `prepare`. Lo
    /// que node rechaza como script y como modulo no sale `esm` en A ni
    /// `script` en B (terser parameters/accept_duplicated_parameters_in_non_strict...).
    #[test]
    fn oxc_rotulo_del_otro_carril_con_semantica() {
        let a = Oxc::new(Preset::CoreDefaults, SourceType::cjs());
        let b = Oxc::new(Preset::CoreDefaults, SourceType::mjs());
        let dup = "function a(b, b) {}\nfunction b({ c: test, c: test }) {}\n";
        assert!(matches!(a.prepare(dup), Outcome::ParseError { esm: false, script: false }));
        assert!(matches!(b.prepare(dup), Outcome::ParseError { esm: false, script: false }));
        let redecl = "let x = 1;\nlet x = 2;\n";
        assert!(matches!(a.prepare(redecl), Outcome::ParseError { esm: false, script: false }));
        assert!(matches!(b.prepare(redecl), Outcome::ParseError { esm: false, script: false }));
        // Lo que el otro carril si acepta sigue rotulado.
        assert!(matches!(a.prepare("export const a = 1;"), Outcome::ParseError { esm: true, script: false }));
        assert!(matches!(b.prepare("with (o) { x; }"), Outcome::ParseError { esm: false, script: true }));
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

// 9e: A1/A2/C1 sobre la muestra fija; hijo de este modulo para armar un Oxc sin optimizar.
#[cfg(test)] #[path = "../../tests/muestra/auditoria.rs"] mod muestra;
