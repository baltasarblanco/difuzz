//! Backends de minificacion: cada uno parsea, imprime y minifica con sus
//! propios crates.
//!
//! Contrato (el de minify.rs): `prepare` devuelve DOS programas impresos con el
//! mismo parser y el mismo printer del backend, el original sin optimizar y el
//! minificado. Asi una divergencia entre ambos solo puede venir del
//! optimizador, nunca del parser o del printer.

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

/// Tercer eje de la celda: que minifier se mide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Minifier {
    Swc,
    Oxc,
}

impl Minifier {
    pub const ALL: &'static str = "swc | oxc";

    pub fn name(self) -> &'static str {
        match self {
            Minifier::Swc => "swc",
            Minifier::Oxc => "oxc",
        }
    }

    /// M21: crate y version del minifier medido, para el meta de los hallazgos.
    /// Mantener igual a Cargo.toml.
    pub fn version(self) -> &'static str {
        match self {
            Minifier::Swc => "swc_ecma_minifier 61.0.8",
            Minifier::Oxc => "oxc_minifier 0.153.0",
        }
    }

    pub fn parse(s: &str) -> Option<Minifier> {
        match s {
            "swc" => Some(Minifier::Swc),
            "oxc" => Some(Minifier::Oxc),
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
        }
    }

    /// El preset `core_defaults` con los nombres preservados (main::names_only).
    pub fn build_core_defaults_keep_names(self, kind: SourceKind) -> Box<dyn Backend> {
        match self {
            Minifier::Swc => Box::new(swc::Swc::core_defaults_keep_names().with_kind(kind, false)),
            Minifier::Oxc => Box::new(oxc::Oxc::core_defaults_keep_names(source_type(kind))),
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
        for m in [Minifier::Swc, Minifier::Oxc] {
            assert_eq!(Minifier::parse(m.name()), Some(m));
        }
        assert_eq!(Minifier::parse("terser"), None);
    }
}
