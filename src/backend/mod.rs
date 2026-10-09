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

    /// B5 (decision pendiente del usuario): el default de `--minifier`.
    pub const POR_DEFECTO: Minifier = Minifier::Swc;

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

    /// El preset `core_defaults` con los nombres preservados (main::names_only).
    pub fn build_core_defaults_keep_names(self, kind: SourceKind) -> Box<dyn Backend> {
        match self {
            Minifier::Swc => Box::new(swc::Swc::core_defaults_keep_names().with_kind(kind, false)),
            Minifier::Oxc => Box::new(oxc::Oxc::core_defaults_keep_names(source_type(kind))),
            #[cfg(test)]
            Minifier::Identidad => Box::new(identidad::Identidad { kind }),
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

    /// 7b (aceptacion): el backend falso funciona por el camino normal
    /// (`Preset::build` + `minify::prepare`, con el analisis de difuzz) en los
    /// dos carriles: minificado = original, y los flags estaticos salen igual.
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
            assert!(static_flags.contains(&"with") && static_flags.contains(&"callee"), "{static_flags:?}");
        }
        let cfg = Preset::CoreDefaults.build(Minifier::Identidad, SourceKind::Module);
        let Outcome::Ok { original, minified, .. } = prepare("export const a = 1 + 2;\n", &cfg) else { panic!("modulo") };
        assert_eq!(original, minified);
        assert!(original.contains("1 + 2") || original.contains("1+2"), "{original}");
    }
}
