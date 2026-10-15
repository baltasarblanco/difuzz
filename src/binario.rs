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
//!
//! M3 (revision de los bloques 4-6): entre `comprobar` y el spawn quedaba una ventana (TOCTOU), y un
//! `touch` o una recompilacion del propio binario cortaba la corrida aunque no
//! hiciera falta. Ahora (solo Linux, como el subreaper):
//! - Este mismo binario se lanza por `/proc/self/exe` (`propio`): el kernel ejecuta el
//!   inodo con el que arranco este proceso, aunque la ruta ya apunte a otro archivo.
//!   Reescribirlo en el lugar falla mientras corre (ETXTBSY), asi que no se vigila.
//! - El oraculo se abre al fijar y se lanza por `/proc/self/fd/N` (`para_lanzar`): un
//!   reemplazo por rename (otro inodo, como `cargo build`) ya no cambia lo que se
//!   ejecuta. Se sigue vigilando ese inodo (fstat): si lo reescriben en el lugar
//!   (tamano o mtime; un `touch` tambien, conservador) se corta como antes. Ahi la
//!   ventana entre fstat y exec sigue existiendo (sin ETXTBSY si no hay un hijo vivo).
//!
//! Sin `/proc` (otro sistema) se lanza por ruta y se vigila como antes.

use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
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

fn huella(m: &std::fs::Metadata) -> Huella {
    Huella { dev: m.dev(), ino: m.ino(), len: m.len(), mtime: m.mtime(), mtime_nsec: m.mtime_nsec() }
}

/// M3: directorio de los fd de este proceso. Sin el, se lanza y se vigila por ruta.
const PROC_FD: &str = "/proc/self/fd";

/// M3: un ejecutable vigilado. Con `/proc`, `archivo` mantiene vivo el inodo del
/// arranque (se lanza por el, ver `para_lanzar`) y la huella se mira con fstat;
/// sin `/proc`, `archivo` es `None` y se mira la ruta, como antes.
struct Fijo {
    ruta: PathBuf,
    archivo: Option<File>,
    antes: Huella,
}

impl Fijo {
    fn ahora(&self) -> std::io::Result<Huella> {
        match &self.archivo {
            Some(f) => f.metadata().map(|m| huella(&m)),
            None => std::fs::metadata(&self.ruta).map(|m| huella(&m)),
        }
    }
}

/// Las huellas de arranque y, si ya se detecto, el primer cambio (pegajoso).
pub struct Vigilante {
    fijas: Vec<Fijo>,
    cambio: Mutex<Option<String>>,
}

impl Vigilante {
    /// Las rutas que no se pueden leer ahora no se vigilan (devuelve cuales).
    pub fn new(rutas: &[PathBuf]) -> (Vigilante, Vec<String>) {
        Vigilante::con_fd(rutas, Path::new(PROC_FD).is_dir())
    }

    /// `con_fd`: abrir cada ejecutable y vigilar (y lanzar) ese inodo (M3).
    fn con_fd(rutas: &[PathBuf], con_fd: bool) -> (Vigilante, Vec<String>) {
        let mut fijas = Vec::new();
        let mut sin = Vec::new();
        for p in rutas {
            let fijo = if con_fd {
                File::open(p).and_then(|f| Ok(Fijo { ruta: p.clone(), antes: huella(&f.metadata()?), archivo: Some(f) }))
            } else {
                std::fs::metadata(p).map(|m| Fijo { ruta: p.clone(), archivo: None, antes: huella(&m) })
            };
            match fijo {
                Ok(f) => fijas.push(f),
                Err(e) => sin.push(format!("{}: {e}", p.display())),
            }
        }
        (Vigilante { fijas, cambio: Mutex::new(None) }, sin)
    }

    /// M3: con que ruta lanzar `p`: `/proc/self/fd/N` si se vigila por fd (siempre
    /// el inodo del arranque), si no la misma ruta.
    fn para_lanzar(&self, p: &Path) -> PathBuf {
        self.fijas
            .iter()
            .find(|f| f.ruta == p)
            .and_then(|f| f.archivo.as_ref())
            .map_or_else(|| p.to_path_buf(), |a| Path::new(PROC_FD).join(a.as_raw_fd().to_string()))
    }

    /// `Err(motivo)` si algun ejecutable ya no es el del arranque. El primer
    /// cambio queda fijo: despues, siempre `Err` con el mismo motivo.
    pub fn comprobar(&self) -> Result<(), String> {
        let mut cambio = self.cambio.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = cambio.as_ref() {
            return Err(m.clone());
        }
        for fijo in &self.fijas {
            let (p, antes) = (&fijo.ruta, &fijo.antes);
            let detalle = match fijo.ahora() {
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

/// M3: `/proc/self/exe`, el inodo con el que arranco este proceso (solo Linux).
const PROC_EXE: &str = "/proc/self/exe";

/// M3: con que lanzar este mismo binario (`--preparar`, `--analizar`):
/// `/proc/self/exe` si existe (el codigo de esta corrida aunque recompilen), si no
/// la ruta de `current_exe`.
pub fn propio() -> PathBuf {
    if Path::new(PROC_EXE).exists() {
        return PathBuf::from(PROC_EXE);
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("difuzz"))
}

/// main, una vez al arrancar: vigila estos ejecutables. Devuelve las rutas que
/// no pudo leer (main las avisa). M3: este mismo binario no se vigila si se lanza
/// por `/proc/self/exe` (`propio`): ni un rename ni un `touch` cambian lo que se
/// ejecuta, y reescribirlo en el lugar falla mientras corre (ETXTBSY).
pub fn fijar(rutas: &[PathBuf]) -> Vec<String> {
    let mio = (propio() == Path::new(PROC_EXE)).then(std::env::current_exe).and_then(Result::ok);
    let rutas: Vec<PathBuf> = rutas.iter().filter(|p| mio.as_deref() != Some(p.as_path())).cloned().collect();
    let (v, sin) = Vigilante::new(&rutas);
    let _ = VIGILANTE.set(v);
    sin
}

/// M3: con que ruta lanzar el ejecutable `p` (el oraculo): `/proc/self/fd/N` si
/// `fijar` lo abrio (el inodo del arranque); si no, `p`.
pub fn para_lanzar(p: &Path) -> PathBuf {
    VIGILANTE.get().map_or_else(|| p.to_path_buf(), |v| v.para_lanzar(p))
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
    /// reescribirlo en el lugar se detecta; el primer cambio queda fijo. M3: es el
    /// modo por ruta (sin `/proc`); el modo por fd esta en `por_fd_lanza_el_inodo_del_arranque`.
    #[test]
    fn detecta_binario_reemplazado() {
        let d = std::env::temp_dir().join(format!("difuzz-test-binario-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("difuzz");
        let oraculo = d.join("difuzz-oraculo");
        std::fs::write(&exe, "version 1").unwrap();
        std::fs::write(&oraculo, "oraculo 1").unwrap();
        let por_ruta = |r: &[std::path::PathBuf]| Vigilante::con_fd(r, false);
        let (v, sin) = por_ruta(&[exe.clone(), oraculo.clone(), d.join("no-existe")]);
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
        let (v, _) = por_ruta(&[exe.clone(), oraculo.clone()]);
        std::fs::write(&oraculo, "oraculo 2 mas largo").unwrap();
        let e = v.comprobar().unwrap_err();
        assert!(e.contains("difuzz-oraculo cambio durante la corrida: reescrito (tamano 9 -> 19"), "{e}");
        let (v, _) = por_ruta(std::slice::from_ref(&exe));
        std::fs::remove_file(&exe).unwrap();
        assert!(v.comprobar().unwrap_err().contains("ya no se puede leer"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M3 (solo Linux): con el ejecutable abierto al fijar, un reemplazo por rename (o
    /// borrarlo) no corta nada y lo que se lanza por `/proc/self/fd/N` es el inodo del
    /// arranque. Reescribirlo en el lugar (mismo inodo) se sigue detectando.
    #[test]
    fn por_fd_lanza_el_inodo_del_arranque() {
        if !std::path::Path::new(super::PROC_FD).is_dir() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-binario-fd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Un ELF de verdad: exec por /proc/self/fd/N de un fd O_CLOEXEC no sirve con `#!`.
        let ora = d.join("oraculo");
        std::fs::copy("/bin/true", &ora).unwrap();
        let (v, sin) = Vigilante::new(std::slice::from_ref(&ora));
        assert!(sin.is_empty(), "{sin:?}");
        let lanzar = v.para_lanzar(&ora);
        assert!(lanzar.starts_with(super::PROC_FD), "{}", lanzar.display());
        assert_eq!(v.para_lanzar(&d.join("otro")), d.join("otro"));
        // Reemplazo por rename con algo que falla y deja marca: no corta y no se ejecuta.
        let marca = d.join("marca");
        let nuevo = d.join("oraculo.nuevo");
        std::fs::write(&nuevo, format!("#!/bin/sh\ntouch {}\nexit 3\n", marca.display())).unwrap();
        std::fs::set_permissions(&nuevo, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::rename(&nuevo, &ora).unwrap();
        assert_eq!(v.comprobar(), Ok(()));
        // ETXTBSY: otro hilo del test que hace fork mientras el archivo recien escrito
        // estaba abierto lo retiene un instante; se reintenta.
        let correr = |p: &std::path::Path| {
            for _ in 0..100 {
                match std::process::Command::new(p).status() {
                    Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    r => return r.unwrap(),
                }
            }
            panic!("ETXTBSY persistente: {}", p.display());
        };
        let st = correr(&lanzar);
        assert!(st.success() && !marca.exists(), "{st:?}");
        // Por ruta si se ejecuta el reemplazo (sin esto el test no prueba nada).
        assert_eq!(correr(&ora).code(), Some(3));
        // Borrado: el inodo sigue vivo por el fd.
        std::fs::remove_file(&ora).unwrap();
        assert_eq!(v.comprobar(), Ok(()));
        assert!(correr(&lanzar).success());
        // Reescrito en el lugar (mismo inodo): se detecta.
        let f = d.join("en-el-lugar");
        std::fs::write(&f, "uno").unwrap();
        let (v, _) = Vigilante::new(std::slice::from_ref(&f));
        std::fs::write(&f, "uno mas largo").unwrap();
        let e = v.comprobar().unwrap_err();
        assert!(e.contains("en-el-lugar cambio durante la corrida: reescrito (tamano 3 -> 13"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M3: el propio binario no se vigila porque no se puede reescribir en el lugar
    /// mientras corre (ETXTBSY) y se lanza por `/proc/self/exe`.
    #[test]
    fn el_propio_binario_no_se_reescribe_mientras_corre() {
        if super::propio() != std::path::Path::new(super::PROC_EXE) {
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let e = std::fs::OpenOptions::new().write(true).open(&exe).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ETXTBSY), "{e}");
    }
}
