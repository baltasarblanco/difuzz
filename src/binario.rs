//! R1-H18: el ejecutable de difuzz (o su oraculo) cambio a mitad de corrida.
//!
//! Los hijos (`--preparar`, `--analizar`, el oraculo) se lanzan por ruta, de
//! forma perezosa, y se re-lanzan cuando mueren. Si `cargo build` reemplaza el
//! binario mientras una corrida esta en curso, el hijo que se lanza despues es
//! OTRO codigo y la corrida mezcla versiones sin avisar.
//!
//! main fija al arrancar la huella (dispositivo, inodo, tamano y mtime) de este
//! ejecutable y del oraculo (`fijar`). Antes de lanzar cada hijo se comparan
//! (`comprobar`, en `subproceso::Remoto::lanzar` y `analisis::lanzar`). Si
//! alguna cambio: ese hijo no se lanza (el que lo pidio cae a su respaldo en
//! proceso, que es el codigo de esta corrida; el oraculo no tiene respaldo y el
//! caso queda como panic), se avisa una vez, los workers no toman otro caso y
//! la corrida cierra como `incompleta` con `binario_cambiado:` en CORRIDA.txt.
//! Un hijo que ya estaba vivo sigue siendo el binario viejo: no mezcla nada.
//!
//! Sin `fijar` (tests, `--emitir`) no se comprueba nada.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Lo que cambia cuando el archivo se reemplaza (rename: otro inodo) o se
/// reescribe en el lugar (tamano o mtime).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Huella {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
}

fn huella(p: &Path) -> std::io::Result<Huella> {
    let m = std::fs::metadata(p)?;
    Ok(Huella { dev: m.dev(), ino: m.ino(), len: m.len(), mtime: m.mtime(), mtime_nsec: m.mtime_nsec() })
}

/// Las huellas de arranque y, si ya se detecto, el primer cambio (pegajoso).
pub struct Vigilante {
    fijas: Vec<(PathBuf, Huella)>,
    cambio: Mutex<Option<String>>,
}

impl Vigilante {
    /// Las rutas que no se pueden leer ahora no se vigilan (devuelve cuales).
    pub fn new(rutas: &[PathBuf]) -> (Vigilante, Vec<String>) {
        let mut fijas = Vec::new();
        let mut sin = Vec::new();
        for p in rutas {
            match huella(p) {
                Ok(h) => fijas.push((p.clone(), h)),
                Err(e) => sin.push(format!("{}: {e}", p.display())),
            }
        }
        (Vigilante { fijas, cambio: Mutex::new(None) }, sin)
    }

    /// `Err(motivo)` si algun ejecutable ya no es el del arranque. El primer
    /// cambio queda fijo: despues, siempre `Err` con el mismo motivo.
    pub fn comprobar(&self) -> Result<(), String> {
        let mut cambio = self.cambio.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = cambio.as_ref() {
            return Err(m.clone());
        }
        for (p, antes) in &self.fijas {
            let detalle = match huella(p) {
                Ok(ahora) if ahora == *antes => continue,
                Ok(ahora) if ahora.ino != antes.ino || ahora.dev != antes.dev => {
                    format!("reemplazado (inodo {} -> {})", antes.ino, ahora.ino)
                }
                Ok(ahora) => format!(
                    "reescrito (tamano {} -> {}, mtime {}.{:09} -> {}.{:09})",
                    antes.len, ahora.len, antes.mtime, antes.mtime_nsec, ahora.mtime, ahora.mtime_nsec
                ),
                Err(e) => format!("ya no se puede leer ({e})"),
            };
            let m = format!("{} cambio durante la corrida: {detalle}", p.display());
            *cambio = Some(m.clone());
            return Err(m);
        }
        Ok(())
    }

    pub fn cambio(&self) -> Option<String> {
        self.cambio.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

static VIGILANTE: OnceLock<Vigilante> = OnceLock::new();
static AVISADO: AtomicBool = AtomicBool::new(false);

/// main, una vez al arrancar: vigila estos ejecutables. Devuelve las rutas que
/// no pudo leer (main las avisa).
pub fn fijar(rutas: &[PathBuf]) -> Vec<String> {
    let (v, sin) = Vigilante::new(rutas);
    let _ = VIGILANTE.set(v);
    sin
}

/// Antes de lanzar un hijo. El primer `Err` se avisa por stderr (una vez).
pub fn comprobar() -> Result<(), String> {
    let Some(v) = VIGILANTE.get() else {
        return Ok(());
    };
    v.comprobar().inspect_err(|m| {
        if !AVISADO.swap(true, Ordering::SeqCst) {
            eprintln!(
                "ERROR: {m}. Un hijo lanzado ahora seria otro codigo: no lo lanzo, termino los casos en curso y corto la corrida (CORRIDA.txt: incompleta, binario_cambiado)"
            );
        }
    })
}

/// El cambio detectado, si hubo (main: cortar la corrida y marcarla).
pub fn cambio() -> Option<String> {
    VIGILANTE.get().and_then(Vigilante::cambio)
}

#[cfg(test)]
mod tests {
    use super::Vigilante;

    /// R1-H18: reemplazar el archivo (como `cargo build`: otro inodo) o
    /// reescribirlo en el lugar se detecta; el primer cambio queda fijo.
    #[test]
    fn detecta_binario_reemplazado() {
        let d = std::env::temp_dir().join(format!("difuzz-test-binario-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("difuzz");
        let oraculo = d.join("difuzz-oraculo");
        std::fs::write(&exe, "version 1").unwrap();
        std::fs::write(&oraculo, "oraculo 1").unwrap();
        let (v, sin) = Vigilante::new(&[exe.clone(), oraculo.clone(), d.join("no-existe")]);
        assert_eq!(sin.len(), 1, "{sin:?}");
        assert_eq!(v.comprobar(), Ok(()));
        assert_eq!(v.cambio(), None);
        // Reemplazo por rename: otro inodo, aunque el contenido mida lo mismo.
        let nuevo = d.join("difuzz.nuevo");
        std::fs::write(&nuevo, "version 2").unwrap();
        std::fs::rename(&nuevo, &exe).unwrap();
        let e = v.comprobar().unwrap_err();
        assert!(e.contains("difuzz cambio durante la corrida: reemplazado (inodo"), "{e}");
        // Pegajoso: aunque vuelva a estar igual, la corrida ya quedo mezclada.
        assert_eq!(v.comprobar(), Err(e.clone()));
        assert_eq!(v.cambio(), Some(e));

        // Reescrito en el lugar (mismo inodo, otro tamano), y borrado.
        let (v, _) = Vigilante::new(&[exe.clone(), oraculo.clone()]);
        std::fs::write(&oraculo, "oraculo 2 mas largo").unwrap();
        let e = v.comprobar().unwrap_err();
        assert!(e.contains("difuzz-oraculo cambio durante la corrida: reescrito (tamano 9 -> 19"), "{e}");
        let (v, _) = Vigilante::new(std::slice::from_ref(&exe));
        std::fs::remove_file(&exe).unwrap();
        assert!(v.comprobar().unwrap_err().contains("ya no se puede leer"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
