//! R2-H05: marca de corrida, `<out>/<celda>/CORRIDA.txt`.
//!
//! Se escribe con `estado: incompleta` ANTES de que el reporter borre los
//! hallazgos de la corrida anterior, y solo al final pasa a `completa` (o a
//! `interrumpida` si llego SIGINT/SIGTERM). Un SIGKILL, un abort o una segunda
//! senal (`_exit`) no corren nada: la marca queda en `incompleta`. Para un
//! lector, todo lo que no sea `estado: completa` (primera linea) es una corrida
//! que no termino: `div/`, `descartes.tsv` y el resto pueden estar a medias.
//! Cada cambio de estado es temporal + rename: la marca nunca queda cortada.

use std::path::{Path, PathBuf};

pub const ARCHIVO: &str = "CORRIDA.txt";

pub struct Corrida {
    path: PathBuf,
    /// Lo que no cambia entre estados (celda, inicio, versiones, ...).
    datos: String,
}

impl Corrida {
    /// Crea `dir` si hace falta y deja la marca en `incompleta`. Si no se
    /// puede escribir, avisa y sigue: el reporter falla en el mismo directorio
    /// y main corta ahi (M27 a).
    pub fn iniciar(dir: &Path, datos: &[(&str, String)]) -> Corrida {
        let mut texto = format!("inicio: {}\npid: {}\n", utc(ahora()), std::process::id());
        for (k, v) in datos {
            texto.push_str(&format!("{k}: {v}\n"));
        }
        let c = Corrida { path: dir.join(ARCHIVO), datos: texto };
        if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| c.escribir("incompleta", &[])) {
            eprintln!("no pude escribir {}: {e}", c.path.display());
        }
        c
    }

    /// Cierra la marca: `estado` es `completa`, `interrumpida` o `incompleta`
    /// (p. ej. si el reporter perdio escrituras). `notas` va al final (la linea
    /// de stats, cuantos casos se procesaron).
    pub fn cerrar(&self, estado: &str, notas: &[String]) {
        let mut extra = vec![format!("fin: {}", utc(ahora()))];
        extra.extend_from_slice(notas);
        if let Err(e) = self.escribir(estado, &extra) {
            eprintln!("no pude cerrar {}: {e}", self.path.display());
        }
    }

    fn escribir(&self, estado: &str, extra: &[String]) -> std::io::Result<()> {
        let mut texto = format!(
            "estado: {estado}\n# difuzz: solo 'estado: completa' es una corrida terminada; incompleta = en curso, cortada o muerta sin cierre\n{}",
            self.datos
        );
        for l in extra {
            texto.push_str(l);
            texto.push('\n');
        }
        let tmp = PathBuf::from(format!("{}.tmp-{}", self.path.display(), std::process::id()));
        std::fs::write(&tmp, texto)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// El estado de la corrida en `dir` (`completa`, `interrumpida`,
/// `incompleta`), o `None` si no hay marca (salida de un difuzz anterior).
pub fn estado(dir: &Path) -> Option<String> {
    let texto = std::fs::read_to_string(dir.join(ARCHIVO)).ok()?;
    texto.lines().next()?.strip_prefix("estado: ").map(str::to_string)
}

fn ahora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Segundos desde 1970 a `AAAA-MM-DDTHH:MM:SSZ` (UTC), sin crates: dias a
/// fecha civil con el algoritmo `civil_from_days` de H. Hinnant.
fn utc(secs: u64) -> String {
    let (dias, s) = ((secs / 86_400) as i64, secs % 86_400);
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Referencias de `date -u -d @N +%FT%TZ`.
    #[test]
    fn fechas_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_791_460_496), "2026-10-08T11:54:56Z");
        assert_eq!(utc(4_107_542_399), "2100-02-28T23:59:59Z");
    }

    /// R2-H05: la marca arranca en incompleta (lo que queda si el proceso
    /// muere sin cerrar) y solo `cerrar` la cambia; el resto de los datos se
    /// conserva.
    #[test]
    fn incompleta_hasta_cerrar() {
        let d = std::env::temp_dir().join(format!("difuzz-test-corrida-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(estado(&d), None);
        let c = Corrida::iniciar(&d.join("celda"), &[("celda", "oxc+x".into())]);
        assert_eq!(estado(&d.join("celda")).as_deref(), Some("incompleta"));
        // Un corte sin cierre (SIGKILL): no hay nada mas que lo cambie.
        drop(c);
        assert_eq!(estado(&d.join("celda")).as_deref(), Some("incompleta"));
        let c = Corrida::iniciar(&d.join("celda"), &[("celda", "oxc+x".into())]);
        c.cerrar("interrumpida", &["procesados: 3 de 10".into()]);
        let t = std::fs::read_to_string(d.join("celda").join(ARCHIVO)).unwrap();
        assert!(t.starts_with("estado: interrumpida\n"), "{t}");
        for want in ["\ncelda: oxc+x\n", "\ninicio: ", "\nfin: ", "\nprocesados: 3 de 10\n"] {
            assert!(t.contains(want), "falta {want:?} en\n{t}");
        }
        c.cerrar("completa", &[]);
        assert_eq!(estado(&d.join("celda")).as_deref(), Some("completa"));
        // Sin temporales sueltos.
        let n = std::fs::read_dir(d.join("celda")).unwrap().count();
        assert_eq!(n, 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
