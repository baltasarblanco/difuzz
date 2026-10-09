//! 7b: backend falso de prueba (solo en tests). Parsea e imprime con oxc y
//! devuelve el original como minificado. Prueba de aceptacion del bloque 3: un
//! backend nuevo se agrega tocando solo `src/backend/` (este archivo y su
//! entrada en `mod.rs`) y no hereda nada de otro backend (asunciones, backlog,
//! abreviatura de `--cruce`). Basado en el de la revision 1
//! (`revision-1/pruebas/b1-identidad.rs`), con el `Outcome` de hoy.

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_span::SourceType;

use super::{Backend, Politica};
use crate::minify::{Outcome, SourceKind};

/// Lo que difuzz sabe de este backend: nada propio. Sin asunciones, sin
/// backlog, sin re-chequeos; los chequeos estaticos los hace el analisis.
pub const POLITICA: Politica = Politica {
    abreviatura: "id",
    razones: &[],
    rutas: &[],
    backlog: None,
    rechequeos: &[],
    solo_sloppy: None,
    chequeos_propios: false,
    flags_estaticos: None,
};

pub struct Identidad {
    pub kind: SourceKind,
}

impl Backend for Identidad {
    fn prepare(&self, src: &str) -> Outcome {
        let st = match self.kind {
            SourceKind::Script => SourceType::cjs(),
            SourceKind::Module => SourceType::mjs(),
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, src, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() {
            return Outcome::ParseError { esm: false, script: false };
        }
        let original = Codegen::new().with_options(CodegenOptions::minify()).build(&parsed.program).code;
        // Como oxc: en el hijo minificador los chequeos los pide el analisis.
        let (static_flags, dead_writes, analisis) = if crate::analisis::checks_afuera() {
            (Vec::new(), (0, 0), Default::default())
        } else {
            crate::analisis::atrapar(|| super::swc::checks_on_text(src, &original, &original, self.kind, super::Minifier::Identidad)).unwrap_or_else(|e| {
                crate::analisis::fallo_en_backend(e);
                (vec![crate::analisis::FALLO_ANALISIS], (0, 0), crate::checks::Analisis::PERDIDO)
            })
        };
        Outcome::Ok { minified: original.clone(), original, static_flags, dead_writes, analisis }
    }
}
