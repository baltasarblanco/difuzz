//! Divergencias esperadas: no son hallazgos nuevos. Tres fuentes, en orden.
//!
//! 1. Sensor: flags del preload (runtime) y de checks::static_flags (AST) que
//!    dicen que el programa usa algo que AGENTS.md del minifier declara asumido.
//! 2. Ruta: asunciones conocidas por nombre de fixture (tabla vieja; queda hasta
//!    confirmar que el sensor la cubre).
//! 3. El backlog que SWC ya conoce: tests/TODO.txt y tests/postponed.txt listan
//!    fixtures de terser que el minifier sabe que no cumple. Se leen del corpus.
//!
//! Una divergencia conocida se guarda igual (en `known/`, no en `div/`) y se
//! cuenta aparte: no se pierde, pero no compite por atencion. Su sintoma se
//! persiste: si un caso conocido diverge distinto en otra corrida, vuelve a
//! `div/` (known-sintomas.tsv).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
];

/// Lo mismo para oxc, contra crates/oxc_minifier/docs/ASSUMPTIONS.md (tag
/// crates_v0.153.0). Asunciones de oxc SIN flag todavia (sus divergencias caen
/// en div/): `document.all`, `with`, longitud maxima de Array/String/TypedArray,
/// coercion que lanza con Symbol (`Math.abs(Symbol())`), `extends` que lanza,
/// `var` de un eval directo usada afuera, globals conocidos que existen y se
/// leen sin efectos (`window`, `document` no existen en node), `arguments`
/// global, y escrituras de propiedades sobre bindings no usados (setters en
/// prototipos: solo cubierto si el preload ve `builtins`).
/// Flags de SWC que oxc NO declara como asuncion y por eso no van aca:
/// `proxy` (property_read_side_effects es All por defecto), `callee`,
/// `redefine_global`.
const FLAG_REASONS_OXC: &[(&str, &str)] = &[
    ("fn_tostring", "oxc: Function.prototype.toString is not relied on"),
    ("builtins", "oxc: No Monkey-Patching Built-ins"),
    ("tdz", "oxc: No TDZ Violation"),
    ("coercion", "oxc: Coercion Methods Are Pure"),
    ("nombres", "oxc: No Reliance on Function.prototype.name (keep_names apagado en core_defaults)"),
    // No es asuncion documentada de oxc: el contenido de .stack (posiciones,
    // nombres) cambia con cualquier minificacion. Decision de difuzz, VERIFICAR.
    ("stack", "difuzz: el contenido de Error.prototype.stack cambia al minificar (oxc no lo documenta)"),
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

pub struct Known {
    backlog: HashSet<String>,
    symptoms: Symptoms,
    /// Tabla de razones del backend medido (SWC por defecto).
    reasons: &'static [(&'static str, &'static str)],
    /// Asunciones por ruta: son de SWC.
    paths: &'static [(&'static str, &'static str)],
}

impl Known {
    /// Lee los backlogs si el corpus es `crates/swc_ecma_minifier/tests`. Si no
    /// estan, el set queda vacio y solo aplican las asunciones. `oracle_id`
    /// identifica preload + config: sintomas de otro oraculo no aplican.
    pub fn load(corpus: &Path, out: &Path, oracle_id: u64) -> Known {
        let mut backlog = HashSet::new();
        for list in ["TODO.txt", "postponed.txt"] {
            let Ok(text) = std::fs::read_to_string(corpus.join(list)) else {
                continue;
            };
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                backlog.insert(format!("terser/compress/{line}"));
            }
        }
        Known {
            backlog,
            symptoms: Symptoms::load(out, oracle_id),
            reasons: FLAG_REASONS,
            paths: ASSUMPTIONS,
        }
    }

    /// Las razones conocidas dependen del backend: el backlog (TODO.txt,
    /// postponed.txt) y las asunciones por ruta son de SWC; oxc tiene su tabla.
    pub fn with_backend(mut self, minifier: crate::backend::Minifier) -> Known {
        if minifier == crate::backend::Minifier::Oxc {
            self.backlog.clear();
            self.reasons = FLAG_REASONS_OXC;
            self.paths = &[];
        }
        self
    }

    pub fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    pub fn classify(&self, rel: &Path, flags: &[&str]) -> Option<String> {
        if let Some((_, why)) = self.reasons.iter().find(|(f, _)| flags.contains(f)) {
            return Some(format!("sensor, asuncion: {why}"));
        }
        // Una variante (mutate.rs, `caso__mutK`) hereda el backlog de su caso.
        let rel = crate::mutate::rel_base(&rel.to_string_lossy());
        if let Some((_, why)) = self.paths.iter().find(|(p, _)| rel.starts_with(p)) {
            return Some(format!("ruta, {why}"));
        }
        if self.backlog.contains(&rel) {
            return Some("backlog de SWC (tests/TODO.txt o tests/postponed.txt)".into());
        }
        None
    }

    /// `Err(anterior)` si el caso ya tenia otro sintoma registrado. El anterior
    /// no se pisa: el caso sigue saliendo en `div/` hasta que alguien lo mire y
    /// borre su linea de known-sintomas.tsv.
    pub fn record_symptom(&self, rel: &Path, hash: u64) -> Result<(), u64> {
        self.symptoms.record(&rel.to_string_lossy(), hash)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.symptoms.save()
    }
}

/// FNV-1a de 64 bits sobre las partes, separadas por un 0. Estable entre
/// versiones de Rust (DefaultHasher no lo garantiza).
pub fn fnv1a(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// `<out>/known-sintomas.tsv`: `caso<TAB>hash` por linea, bajo un encabezado
/// con el id del oraculo. Sobrevive entre corridas (el reporter no lo borra).
struct Symptoms {
    path: PathBuf,
    header: String,
    table: Mutex<HashMap<String, u64>>,
}

impl Symptoms {
    fn load(out: &Path, oracle_id: u64) -> Symptoms {
        let path = out.join("known-sintomas.tsv");
        let header = format!("# difuzz known-sintomas, oraculo {oracle_id:016x}");
        let mut table = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut lines = text.lines();
            if lines.next() == Some(header.as_str()) {
                for line in lines {
                    if let Some((case, h)) = line.split_once('\t') {
                        if let Ok(h) = u64::from_str_radix(h, 16) {
                            table.insert(case.to_string(), h);
                        }
                    }
                }
            }
        }
        Symptoms {
            path,
            header,
            table: Mutex::new(table),
        }
    }

    fn record(&self, case: &str, hash: u64) -> Result<(), u64> {
        let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        match t.get(case) {
            Some(&prev) if prev != hash => Err(prev),
            Some(_) => Ok(()),
            None => {
                t.insert(case.to_string(), hash);
                Ok(())
            }
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<_> = t.iter().collect();
        rows.sort();
        let mut text = format!("{}\n", self.header);
        for (case, h) in rows {
            text.push_str(&format!("{case}\t{h:016x}\n"));
        }
        std::fs::write(&self.path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::{Known, Symptoms};
    use std::path::Path;

    #[test]
    fn clasifica_backlog_asunciones_y_el_resto() {
        let dir = std::env::temp_dir().join(format!("difuzz-known-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n\n").unwrap();
        std::fs::write(dir.join("postponed.txt"), "reduce_vars/unsafe_evaluate_object_2/input.js\n").unwrap();
        let k = Known::load(&dir, &dir, 0);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(k.backlog_len(), 2);
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns/input.js"), &[]).is_some());
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns__mut2/input.js"), &[]).is_some());
        assert!(k
            .classify(Path::new("terser/compress/reduce_vars/unsafe_evaluate_object_2/input.js"), &[])
            .is_some());
        assert!(k
            .classify(
                Path::new(
                    "terser/compress/issue_597/NaN_and_Infinity_should_not_be_replaced_when_they_are_redefined_evaluate/input.js"
                ),
                &["redefine_global"]
            )
            .is_some_and(|w| w.starts_with("sensor")));
        assert!(k.classify(Path::new("terser/compress/issue_2719/warn/input.js"), &[]).is_some());
        assert!(k.classify(Path::new("fixture/issues/8953/input.js"), &[]).is_none());
        let by_flag = k.classify(Path::new("fixture/issues/8953/input.js"), &["tdz"]);
        assert!(by_flag.is_some_and(|w| w.starts_with("sensor")));
    }

    #[test]
    fn razones_dependen_del_backend() {
        use crate::backend::Minifier;
        let dir = std::env::temp_dir().join(format!("difuzz-known-oxc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n").unwrap();
        let k = Known::load(&dir, &dir, 0).with_backend(Minifier::Oxc);
        std::fs::remove_dir_all(&dir).unwrap();

        // El backlog y las rutas son de SWC.
        assert_eq!(k.backlog_len(), 0);
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns/input.js"), &[]).is_none());
        assert!(k.classify(Path::new("terser/compress/issue_2719/warn/input.js"), &[]).is_none());
        // Asuncion de oxc con flag; asunciones de SWC que oxc no declara.
        assert!(k.classify(Path::new("x/input.js"), &["tdz"]).is_some_and(|w| w.contains("oxc")));
        assert!(k.classify(Path::new("x/input.js"), &["proxy"]).is_none());
        assert!(k.classify(Path::new("x/input.js"), &["redefine_global"]).is_none());
    }

    #[test]
    fn sintomas_persisten_y_detectan_cambios() {
        let dir = std::env::temp_dir().join(format!("difuzz-sintomas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Symptoms::load(&dir, 7);
        assert_eq!(s.record("a", 1), Ok(()));
        assert_eq!(s.record("a", 1), Ok(()));
        assert_eq!(s.record("a", 2), Err(1));
        s.save().unwrap();
        assert_eq!(Symptoms::load(&dir, 7).record("a", 2), Err(1));
        // Otro oraculo: los sintomas viejos no aplican.
        assert_eq!(Symptoms::load(&dir, 8).record("a", 2), Ok(()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
