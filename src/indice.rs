//! Indice compartido entre celdas de la matriz y su cruce.
//!
//! Cada celda es un proceso distinto. Para no depender de appends atomicos
//! entre procesos, cada celda escribe su propio archivo,
//! `<dir>/index-<celda>.tsv`, y el cruce los lee todos. Una celda que se
//! vuelve a correr reemplaza solo su archivo. Dentro del proceso, los workers
//! escriben bajo un Mutex, una linea por hallazgo:
//!
//! `caso<TAB>celda<TAB>destino<TAB>kind<TAB>hash16`
//!
//! `caso` es el nombre del directorio del hallazgo (`div/<caso>`), `destino`
//! es div | known | panic | lint y `hash16` el del sintoma (oracle.rs).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const HEADER: &str = "# difuzz indice v1: caso\tcelda\tdestino\tkind\thash";

pub struct Index {
    cell: String,
    file: Mutex<std::fs::File>,
}

impl Index {
    /// Trunca el archivo de esta celda: lo de una corrida anterior de la misma
    /// celda no se mezcla.
    pub fn create(dir: &Path, cell: &str) -> std::io::Result<Index> {
        std::fs::create_dir_all(dir)?;
        let mut file = std::fs::File::create(file_of(dir, cell))?;
        writeln!(file, "{HEADER}")?;
        Ok(Index {
            cell: cell.to_string(),
            file: Mutex::new(file),
        })
    }

    pub fn record(&self, case: &str, dest: &str, kind: &str, hash: u64) {
        let line = format!("{case}\t{}\t{dest}\t{kind}\t{hash:016x}\n", self.cell);
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // Una sola escritura por linea: si dos procesos apuntan por error a la
        // misma celda, al menos las lineas no se intercalan a la mitad.
        if let Err(e) = f.write_all(line.as_bytes()) {
            eprintln!("indice: no pude escribir: {e}");
        }
    }
}

fn file_of(dir: &Path, cell: &str) -> PathBuf {
    dir.join(format!("index-{cell}.tsv"))
}

struct Row {
    case: String,
    cell: String,
    dest: String,
    kind: String,
}

fn read_rows(dir: &Path) -> std::io::Result<(Vec<String>, Vec<Row>)> {
    let mut cells = BTreeSet::new();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(cell) = name.strip_prefix("index-").and_then(|n| n.strip_suffix(".tsv")) else {
            continue;
        };
        cells.insert(cell.to_string());
        let text = std::fs::read_to_string(entry.path())?;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            if let [case, cell, dest, kind, _hash] = f[..] {
                rows.push(Row {
                    case: case.into(),
                    cell: cell.into(),
                    dest: dest.into(),
                    kind: kind.into(),
                });
            }
        }
    }
    // Mismo orden de columnas que scripts/matriz.sh: core_defaults primero y,
    // dentro de cada config, en_funcion antes que tal_cual.
    let mut cells: Vec<String> = cells.into_iter().collect();
    cells.sort_by(|a, b| {
        let (pa, fa) = a.split_once('+').unwrap_or((a, ""));
        let (pb, fb) = b.split_once('+').unwrap_or((b, ""));
        pb.cmp(pa).then(fa.cmp(fb))
    });
    Ok((cells, rows))
}

/// `core_defaults+en_funcion` -> `C.fn`, como en matriz.sh. Lo desconocido
/// queda con su nombre.
fn short(cell: &str) -> String {
    let (p, f) = cell.split_once('+').unwrap_or((cell, ""));
    let p = match p {
        "core_defaults" => "C",
        "carril_a" => "A",
        other => other,
    };
    let f = match f {
        "en_funcion" => "fn",
        "tal_cual" => "tc",
        other => other,
    };
    if f.is_empty() {
        p.to_string()
    } else {
        format!("{p}.{f}")
    }
}

/// La tabla que armaba scripts/matriz.sh: una fila por caso con divergencia
/// (`div/`), X por celda donde diverge, el kind de la primera celda (en orden
/// de columnas) y el caso. Al final, cuantos hallazgos hay por celda y destino.
pub fn cruce(dir: &Path) -> std::io::Result<String> {
    let (cells, rows) = read_rows(dir)?;
    let width = cells.iter().map(|c| short(c).len()).max().unwrap_or(0).max(4) + 1;
    // caso -> (celdas con div, kind por celda)
    let mut by_case: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for r in &rows {
        *counts.entry((r.cell.as_str(), r.dest.as_str())).or_default() += 1;
        if r.dest == "div" {
            by_case
                .entry(r.case.as_str())
                .or_default()
                .entry(r.cell.as_str())
                .or_insert(r.kind.as_str());
        }
    }

    let mut out = String::new();
    out.push_str("=== divergencias por caso: X = diverge en la celda\n");
    for c in &cells {
        out.push_str(&format!("{:<width$}", short(c)));
    }
    out.push_str(&format!(" {:<10} caso\n", "kind"));
    let mut lines: Vec<String> = by_case
        .iter()
        .map(|(case, in_cells)| {
            let mut row = String::new();
            let mut kind = "";
            for c in &cells {
                match in_cells.get(c.as_str()) {
                    Some(k) => {
                        row.push_str(&format!("{:<width$}", "X"));
                        if kind.is_empty() {
                            kind = k;
                        }
                    }
                    None => row.push_str(&format!("{:<width$}", ".")),
                }
            }
            format!("{row} {kind:<10} {case}")
        })
        .collect();
    // Como el `sort -r` del script: primero las filas con X en las primeras columnas.
    lines.sort_by(|a, b| b.cmp(a));
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str("\n=== hallazgos por celda\n");
    for c in &cells {
        let n = |d: &str| counts.get(&(c.as_str(), d)).copied().unwrap_or(0);
        out.push_str(&format!(
            "{c}: div={} known={} panic={} lint={}\n",
            n("div"),
            n("known"),
            n("panic"),
            n("lint")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tres_celdas_dos_casos() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = Index::create(&d, "carril_a+tal_cual").unwrap();
        let cf = Index::create(&d, "core_defaults+en_funcion").unwrap();
        let ct = Index::create(&d, "core_defaults+tal_cual").unwrap();
        a.record("fixture__issues__9739", "div", "Stdout", 1);
        cf.record("fixture__issues__9739", "div", "Stdout", 1);
        cf.record("terser__compress__x", "div", "ThrowNew", 2);
        ct.record("terser__compress__x", "known", "ThrowNew", 2);
        a.record("terser__compress__y", "lint", "EscriturasSobreLiterales", 3);
        drop((a, cf, ct));

        let t = cruce(&d).unwrap();
        let esperado = "\
=== divergencias por caso: X = diverge en la celda
C.fn C.tc A.tc  kind       caso
X    .    X     Stdout     fixture__issues__9739
X    .    .     ThrowNew   terser__compress__x

=== hallazgos por celda
core_defaults+en_funcion: div=2 known=0 panic=0 lint=0
core_defaults+tal_cual: div=0 known=1 panic=0 lint=0
carril_a+tal_cual: div=1 known=0 panic=0 lint=1
";
        assert_eq!(t, esperado);

        // Correr de nuevo una celda reemplaza solo sus filas.
        let a = Index::create(&d, "carril_a+tal_cual").unwrap();
        drop(a);
        let t = cruce(&d).unwrap();
        assert!(t.contains("X    .    .     Stdout     fixture__issues__9739"));
        assert!(t.contains("carril_a+tal_cual: div=0 known=0 panic=0 lint=0"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
