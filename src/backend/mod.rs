//! Backends de minificacion: cada uno parsea, imprime y minifica con sus
//! propios crates.
//!
//! Contrato (el de minify.rs): `prepare` devuelve DOS programas impresos con el
//! mismo parser y el mismo printer del backend, el original sin optimizar y el
//! minificado. Asi una divergencia entre ambos solo puede venir del
//! optimizador, nunca del parser o del printer.
//!
//! 7a/7b (bloque 3): lo que difuzz sabe de cada backend ademas de minificar
//! (sus asunciones, su backlog, sus re-chequeos, su abreviatura en `--cruce`)
//! vive en su `Politica` (`Minifier::politica`), declarada en el archivo del
//! backend. Afuera de `src/backend/` nadie pregunta que backend es.

#[cfg(test)]
pub mod identidad;
pub mod oxc;
pub mod swc;
#[cfg(test)] mod version_tests;

use crate::minify::{Outcome, Preset, SourceKind};

/// Un minifier ya configurado (preset + tipo de fuente).
pub trait Backend {
    /// Parsea `src` una vez, imprime el original, minifica e imprime el
    /// minificado con el mismo printer. Puede entrar en panico:
    /// minify::prepare lo atrapa y lo devuelve como `Outcome::Panic`.
    fn prepare(&self, src: &str) -> Outcome;
}

/// 7a/7b: lo que difuzz sabe de un backend fuera de minificar. Cada `Minifier`
/// declara la suya (`Minifier::politica`, un match exhaustivo: un backend
/// nuevo no compila sin ella). No hay `Default`: cada campo se declara, aunque
/// sea vacio, y nada se hereda de otro backend en silencio.
#[derive(Debug)]
pub struct Politica {
    /// Nombre corto del backend en la tabla de `--cruce` (indice.rs).
    pub abreviatura: &'static str,
    /// (flag, asuncion que viola) contra la documentacion del minifier. Gana el
    /// primero de la tabla que este en los flags (known.rs).
    pub razones: &'static [(&'static str, &'static str)],
    /// (prefijo de ruta relativa al corpus, asuncion): lo que el sensor no ve.
    pub rutas: &'static [(&'static str, &'static str)],
    /// El backlog que el propio minifier declara en su corpus. `None`: no tiene.
    pub backlog: Option<Backlog>,
    /// Flags estaticos que se re-chequean antes de clasificar (clasificar.rs):
    /// sin re-chequeo, un flag que no esta en `razones` no clasifica igual.
    pub rechequeos: &'static [Rechequeo],
    /// M17: el minifier no soporta sloppy. `Some(razon de known/)`: en el carril
    /// A, una divergencia sin asuncion conocida se re-chequea en estricto.
    pub solo_sloppy: Option<&'static str>,
    /// Los chequeos estaticos (checks.rs) los hace el backend sobre su propio
    /// AST de SWC (`true`, swc). `false`: los textos se re-parsean, y con el
    /// minificador en un hijo los pide el hijo de analisis (analisis.rs).
    pub chequeos_propios: bool,
    /// Flags estaticos de las asunciones de ESTE backend, ademas de los comunes
    /// de `checks::static_flags` (oxc: `checks::static_flags_oxc`, M13). `None`:
    /// ninguno, y no se calculan (09-oct: antes los de oxc se calculaban para
    /// todos los backends; deuda del bloque 3).
    pub flags_estaticos: Option<fn(&swc_ecma_ast::Program) -> Vec<&'static str>>,
}

impl Politica {
    /// Decision del usuario (09-oct): el sensor de violaciones del preload (b4-a,
    /// inspector de V8, x1,67 por ejecucion de node) corre solo para los backends
    /// que usan su resultado: los que piden `Rechequeo::Violaciones` (hoy oxc). Sale
    /// de `rechequeos` y no de un campo aparte, para que no puedan desacoplarse.
    pub fn usa_sensor(&self) -> bool {
        self.rechequeos.contains(&Rechequeo::Violaciones)
    }
}

/// 7b: el backlog de un minifier: listas de casos que el minifier sabe que no
/// cumple, en la raiz del corpus.
#[derive(Debug)]
pub struct Backlog {
    /// Archivos en la raiz del corpus, una ruta por linea.
    pub listas: &'static [&'static str],
    /// Prefijo que se antepone a cada linea para tener la ruta relativa al corpus.
    pub prefijo: &'static str,
    /// La razon de `known/` (meta.txt).
    pub razon: &'static str,
}

/// Re-chequeos de flags estaticos que un backend puede pedir (clasificar.rs).
/// La mecanica es la misma para todos (se neutraliza la asuncion con el
/// analisis de difuzz y se re-minifica con el mismo backend); el backend solo
/// declara cuales aplican. El orden en que corren es el de este enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rechequeo {
    /// `class extends <literal>` -> `extends null`.
    Extends,
    /// Los globals que el minifier asume y node no tiene, como stubs.
    GlobalConocido,
    /// `with (o) S` -> `{ o; S }`.
    With,
    /// b4-a: las violaciones que el preload ubico en A (TDZ, coercion de Symbol,
    /// `class extends` invalido; atrapadas o no) -> algo que no lanza
    /// (`recheck::violaciones_matter`). Re-chequea los flags de
    /// `recheck::FLAGS_VIOLACION`.
    Violaciones,
    /// t-principio: `builtins` (runtime): A y B con lo que A piso fijado a su valor
    /// original (`recheck::builtins_matter`).
    Builtins,
    /// t-principio: `coercion` (estatico): las definiciones de valueOf/toString/
    /// `Symbol.toPrimitive` renombradas (`recheck::coercion_matters`).
    Coercion,
    /// 8.3 (runtime, sin flag previo): A y B difieren solo en mensajes de V8 que escriben el
    /// texto de la expresion (`cb is not a function` / `e is not a function`). Se re-minifica
    /// con mangle apagado (`Minifier::build_sin_mangle`; el backend tiene que tenerla) y se
    /// exige que B sea un renombre correcto de eso (`recheck::mensaje_nombre_matters`).
    MensajeNombre,
}

/// Tercer eje de la celda: que minifier se mide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Minifier {
    Swc,
    Oxc,
    /// 7b: backend falso de prueba (minificado = original), solo en tests.
    #[cfg(test)]
    Identidad,
}

impl Minifier {
    pub const ALL: &'static str = "swc | oxc";

    /// Los backends de produccion, en el orden de `--preparar --info`
    /// (subproceso.rs: el oraculo tiene que tener los mismos).
    pub const TODOS: &'static [Minifier] = &[Minifier::Swc, Minifier::Oxc];

    /// B5 (decision del usuario, 09-oct): el default de `--minifier` es oxc, el
    /// objetivo (D1). Cambia el nombre de la celda por defecto.
    pub const POR_DEFECTO: Minifier = Minifier::Oxc;

    pub fn name(self) -> &'static str {
        match self {
            Minifier::Swc => "swc",
            Minifier::Oxc => "oxc",
            #[cfg(test)]
            Minifier::Identidad => "identidad",
        }
    }

    /// M21: crate y version del minifier medido, para el meta de los hallazgos.
    /// Mantener igual a Cargo.toml.
    pub fn version(self) -> &'static str {
        match self {
            Minifier::Swc => "swc_ecma_minifier 61.0.8",
            Minifier::Oxc => "oxc_minifier 0.153.0",
            #[cfg(test)]
            Minifier::Identidad => "identidad (prueba)",
        }
    }

    /// 7a: lo que difuzz sabe de este backend (ver `Politica`).
    pub fn politica(self) -> &'static Politica {
        match self {
            Minifier::Swc => &swc::POLITICA,
            Minifier::Oxc => &oxc::POLITICA,
            #[cfg(test)]
            Minifier::Identidad => &identidad::POLITICA,
        }
    }

    pub fn parse(s: &str) -> Option<Minifier> {
        match s {
            "swc" => Some(Minifier::Swc),
            "oxc" => Some(Minifier::Oxc),
            #[cfg(test)]
            "identidad" => Some(Minifier::Identidad),
            _ => None,
        }
    }

    /// El preset para un tipo de fuente. En SWC el carril cambia el parser y
    /// las opciones de top_level (swc::Swc::with_kind); en oxc es el
    /// `SourceType`: `cjs()` para el carril A (node corre `.cjs`, sloppy) y
    /// `mjs()` para el B.
    pub fn build(self, preset: Preset, kind: SourceKind) -> Box<dyn Backend> {
        match self {
            Minifier::Swc => {
                let base = match preset {
                    Preset::CarrilA => swc::Swc::carril_a(),
                    Preset::CoreDefaults => swc::Swc::core_defaults(),
                };
                Box::new(base.with_kind(kind, preset == Preset::CarrilA))
            }
            Minifier::Oxc => Box::new(oxc::Oxc::new(preset, source_type(kind))),
            #[cfg(test)]
            Minifier::Identidad => Box::new(identidad::Identidad { kind }),
        }
    }

    /// El preset con los nombres preservados (main::names_only), para los
    /// presets que cambian nombres (hoy solo `core_defaults`).
    pub fn build_con_nombres(self, preset: Preset, kind: SourceKind) -> Box<dyn Backend> {
        match self {
            Minifier::Swc => {
                assert!(preset == Preset::CoreDefaults, "swc: la variante con nombres solo existe para core_defaults");
                Box::new(swc::Swc::core_defaults_keep_names().with_kind(kind, false))
            }
            Minifier::Oxc => Box::new(oxc::Oxc::con_nombres(preset, source_type(kind))),
            #[cfg(test)]
            Minifier::Identidad => Box::new(identidad::Identidad { kind }),
        }
    }

    /// 8.3: el preset con mangle apagado y el resto igual (`Rechequeo::MensajeNombre`). `None`:
    /// el backend no la tiene (swc: no la pide su politica; VERIFICAR la API si se agrega).
    pub fn build_sin_mangle(self, preset: Preset, kind: SourceKind) -> Option<Box<dyn Backend>> {
        match self {
            Minifier::Oxc => Some(Box::new(oxc::Oxc::sin_mangle(preset, source_type(kind)))),
            Minifier::Swc => None,
            #[cfg(test)]
            Minifier::Identidad => None,
        }
    }

    /// 8.3 (H1 de la revision del bloque 8): ¿el mangle del preset promete preservar el `.name`
    /// de funciones o de clases (keep_names)? Sale de las opciones que arma el backend, no del
    /// nombre del preset. Con keep_names, renombrar un binding del que sale un `.name` ya es
    /// un bug del minifier (`recheck::mensaje_nombre_matters`).
    pub fn preserva_nombres(self, preset: Preset, kind: SourceKind) -> bool {
        match self {
            Minifier::Swc => {
                let base = match preset {
                    Preset::CarrilA => swc::Swc::carril_a(),
                    Preset::CoreDefaults => swc::Swc::core_defaults(),
                };
                let s = base.with_kind(kind, preset == Preset::CarrilA);
                s.minify.mangle.as_ref().is_some_and(|m| m.keep_fn_names || m.keep_class_names)
            }
            Minifier::Oxc => oxc::Oxc::new(preset, source_type(kind)).preserva_nombres(),
            // No renombra nada.
            #[cfg(test)]
            Minifier::Identidad => false,
        }
    }
}

fn source_type(kind: SourceKind) -> oxc::SourceType {
    match kind {
        SourceKind::Script => oxc::SourceType::cjs(),
        SourceKind::Module => oxc::SourceType::mjs(),
    }
}

#[cfg(test)]
mod tests {
    use super::Minifier;

    #[test]
    fn eje_del_minifier() {
        for m in [Minifier::Swc, Minifier::Oxc, Minifier::Identidad] {
            assert_eq!(Minifier::parse(m.name()), Some(m));
        }
        assert_eq!(Minifier::parse("terser"), None);
    }

    /// 8.3 (H1): keep_names sale de las opciones del backend: carril_a lo promete y
    /// core_defaults no, en los dos carriles y en los dos backends de produccion.
    #[test]
    fn preserva_nombres_por_preset() {
        use crate::minify::{Preset, SourceKind};
        for m in Minifier::TODOS {
            for k in [SourceKind::Script, SourceKind::Module] {
                assert!(m.preserva_nombres(Preset::CarrilA, k), "{m:?} {k:?}");
                assert!(!m.preserva_nombres(Preset::CoreDefaults, k), "{m:?} {k:?}");
            }
        }
        assert!(!Minifier::Identidad.preserva_nombres(Preset::CarrilA, SourceKind::Script));
    }

    /// 7.5: `TODOS`, `ALL` y la linea `--minifier` del USAGE (main.rs) dicen los mismos
    /// backends, a mano en tres lugares. `TODOS` tiene cada backend de produccion y no el de
    /// prueba (el match es exhaustivo: un backend nuevo no compila sin decidir aca si es de
    /// produccion; agregarlo tambien a la lista); `ALL` son sus nombres en ese orden, y el
    /// USAGE muestra `ALL` y el default (`POR_DEFECTO`).
    #[test]
    fn todos_all_y_usage_coinciden() {
        let de_produccion = |m: Minifier| match m {
            Minifier::Swc | Minifier::Oxc => true,
            Minifier::Identidad => false,
        };
        for m in [Minifier::Swc, Minifier::Oxc, Minifier::Identidad] {
            assert_eq!(Minifier::TODOS.contains(&m), de_produccion(m), "{m:?} en TODOS");
            assert_eq!(Minifier::parse(m.name()), Some(m));
        }
        let nombres: Vec<&str> = Minifier::TODOS.iter().map(|m| m.name()).collect();
        assert_eq!(Minifier::ALL, nombres.join(" | "));
        let linea = crate::USAGE
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("--minifier M "))
            .expect("el USAGE no tiene la linea de --minifier");
        let (lista, def) = linea.split_once("(default:").expect("la linea de --minifier sin default");
        assert_eq!(lista.trim(), Minifier::ALL, "USAGE: {linea}");
        assert_eq!(def.trim().trim_end_matches(')').trim(), Minifier::POR_DEFECTO.name(), "USAGE: {linea}");
    }

    /// 7b: cada backend declara lo suyo: abreviaturas distintas, y ninguno
    /// comparte con otro la tabla de asunciones, las rutas o el backlog (un
    /// `..swc::POLITICA` copiaria las referencias y este test lo ve).
    #[test]
    fn cada_backend_declara_lo_suyo() {
        let todos = [Minifier::Swc, Minifier::Oxc, Minifier::Identidad];
        for (i, x) in todos.iter().enumerate() {
            for y in &todos[i + 1..] {
                let (px, py) = (x.politica(), y.politica());
                assert_ne!(px.abreviatura, py.abreviatura, "{x:?} {y:?}");
                let misma = |a: &[(&str, &str)], b: &[(&str, &str)]| !a.is_empty() && std::ptr::eq(a, b);
                assert!(!misma(px.razones, py.razones), "{x:?} y {y:?} comparten razones");
                assert!(!misma(px.rutas, py.rutas), "{x:?} y {y:?} comparten rutas");
                if let (Some(bx), Some(by)) = (&px.backlog, &py.backlog) {
                    assert_ne!(bx.listas, by.listas, "{x:?} y {y:?} comparten backlog");
                }
            }
        }
        // El backlog y las rutas de terser son solo de SWC.
        assert!(Minifier::Swc.politica().backlog.is_some());
        assert!(Minifier::Oxc.politica().backlog.is_none() && Minifier::Identidad.politica().backlog.is_none());
        assert!(Minifier::TODOS.iter().all(|m| !Minifier::Identidad.eq(m)), "identidad no es de produccion");
    }

    /// 09-oct (deuda del bloque 3): los flags estaticos de oxc (`with`,
    /// `global_conocido`, `extends`) los pide solo la politica de oxc. Con swc no
    /// se calculan (ni en proceso ni en el hijo minificador); los comunes, si.
    #[test]
    fn flags_estaticos_por_politica() {
        use crate::minify::{prepare, Outcome, Preset, SourceKind};
        let src = "with (o) { f(arguments.callee); }\nconsole.log(typeof window);\nclass A extends 1 {}\n";
        for (m, esperados) in [
            (Minifier::Swc, &["callee"][..]),
            (Minifier::Oxc, &["callee", "with", "global_conocido", "extends"][..]),
        ] {
            let cfg = Preset::CarrilA.build(m, SourceKind::Script);
            let Outcome::Ok { static_flags, .. } = prepare(src, &cfg) else { panic!("{m:?} no preparo el script") };
            assert_eq!(static_flags, esperados, "{m:?}");
        }
        assert!(Minifier::Swc.politica().flags_estaticos.is_none() && Minifier::Oxc.politica().flags_estaticos.is_some());
    }

    /// 7b (aceptacion): el backend falso funciona por el camino normal
    /// (`Preset::build` + `minify::prepare`, con el analisis de difuzz) en los
    /// dos carriles: minificado = original, y los flags estaticos comunes salen
    /// igual. 09-oct: los de oxc (`with`) ya no, porque identidad no los declara
    /// en su politica (`flags_estaticos: None`): un backend no hereda los de otro.
    #[test]
    fn backend_identidad_funciona() {
        use crate::minify::{prepare, Outcome, Preset, SourceKind};
        for kind in [SourceKind::Script, SourceKind::Module] {
            let cfg = Preset::CarrilA.build(Minifier::Identidad, kind);
            let Outcome::Ok { original, minified, static_flags, .. } = prepare("with (o) { f(arguments.callee); }\n", &cfg) else {
                if kind == SourceKind::Module {
                    continue; // `with` no parsea en modulo
                }
                panic!("identidad tenia que preparar el script");
            };
            assert_eq!(original, minified);
            assert!(!static_flags.contains(&"with") && static_flags.contains(&"callee"), "{static_flags:?}");
        }
        let cfg = Preset::CoreDefaults.build(Minifier::Identidad, SourceKind::Module);
        let Outcome::Ok { original, minified, .. } = prepare("export const a = 1 + 2;\n", &cfg) else { panic!("modulo") };
        assert_eq!(original, minified);
        assert!(original.contains("1 + 2") || original.contains("1+2"), "{original}");
    }
}
