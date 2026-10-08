//! Subreaper (pendiente del fuego, M2): un programa que lanza
//! `child_process.spawn(..., {detached: true})` hace `setsid` y escapa del grupo
//! de procesos que el runner mata (M2) y del timeout; con `stdio: 'inherit'`
//! ademas retiene los pipes y el runner espera READ_GRACE.
//!
//! En Linux difuzz se declara subreaper (`prctl(PR_SET_CHILD_SUBREAPER)`): los
//! descendientes huerfanos pasan a ser hijos de difuzz en vez de init. Despues
//! de cada ejecucion `cosechar` mata y cosecha a los hijos de difuzz que no son
//! suyos. Un hijo legitimo es:
//! - node del runner o el hijo que minifica (subproceso.rs): lider de su grupo
//!   (`process_group(0)`), en la sesion de difuzz;
//! - `node --check`, `node --version`: en el grupo y la sesion de difuzz.
//!
//! Huerfano = hijo de difuzz en otra sesion (hizo `setsid`: `detached`) o en un
//! grupo ajeno del que no es lider (quedo del grupo de un node ya muerto). Esos
//! nunca los espera un `Child::wait`, asi que cosecharlos (waitpid por pid, nunca
//! -1) no le roba el status a nadie. Mientras no se cosecha, el pid no se reusa:
//! leer /proc y matar no tiene carrera.
//!
//! No cubre: un descendiente que hace `setpgid(0,0)` sin `setsid` (queda lider
//! de su grupo en nuestra sesion, igual que un node legitimo). Fuera de Linux,
//! no hace nada (compila igual).

use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVO: AtomicBool = AtomicBool::new(false);

/// Declara a difuzz subreaper. `true` si quedo activo.
pub fn activar() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl con un entero; no toca memoria de este proceso.
        let r = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong, 0, 0, 0) };
        if r == 0 {
            ACTIVO.store(true, Ordering::SeqCst);
            return true;
        }
    }
    false
}

pub fn activo() -> bool {
    ACTIVO.load(Ordering::SeqCst)
}

/// Mata y cosecha los huerfanos adoptados. Devuelve cuantos cosecho. Sin
/// subreaper activo, no hace nada.
pub fn cosechar() -> usize {
    if !activo() {
        return 0;
    }
    #[cfg(target_os = "linux")]
    {
        linux::cosechar()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
mod linux {
    /// `(pgrp, session)` de `/proc/<pid>/stat` (campos 5 y 6, contando desde el
    /// que sigue al `)` final del nombre, que puede tener espacios).
    pub(super) fn grupo_y_sesion(pid: i32) -> Option<(i32, i32)> {
        let st = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let resto = &st[st.rfind(')')? + 1..];
        let mut f = resto.split_whitespace();
        // estado, ppid, pgrp, session
        let pgrp = f.nth(2)?.parse().ok()?;
        let sesion = f.next()?.parse().ok()?;
        Some((pgrp, sesion))
    }

    /// Hijos directos de este proceso: `/proc/self/task/*/children`
    /// (CONFIG_PROC_CHILDREN). Si no existe, recorre /proc buscando ppid.
    fn hijos() -> Vec<i32> {
        let mut v = Vec::new();
        let mut alguno = false;
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for t in tasks.flatten() {
                if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
                    alguno = true;
                    v.extend(s.split_whitespace().filter_map(|p| p.parse::<i32>().ok()));
                }
            }
        }
        if alguno {
            return v;
        }
        let yo = std::process::id() as i32;
        let Ok(procs) = std::fs::read_dir("/proc") else {
            return v;
        };
        for p in procs.flatten() {
            let Some(pid) = p.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            let ppid = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|st| st[st.rfind(')')? + 1..].split_whitespace().nth(1)?.parse::<i32>().ok());
            if ppid == Some(yo) {
                v.push(pid);
            }
        }
        v
    }

    pub(super) fn cosechar() -> usize {
        // SAFETY: getsid/getpgrp no toman punteros.
        let (mi_sesion, mi_grupo) = unsafe { (libc::getsid(0), libc::getpgrp()) };
        let mut n = 0;
        // Matar a un huerfano puede dejar huerfanos a sus hijos: se repite hasta
        // que no aparezca ninguno (con tope).
        for _ in 0..8 {
            let mut esta_vuelta = 0;
            for pid in hijos() {
                let Some((grupo, sesion)) = grupo_y_sesion(pid) else {
                    continue;
                };
                if !es_huerfano(pid, grupo, sesion, mi_grupo, mi_sesion) {
                    continue;
                }
                // SAFETY: pid es hijo nuestro sin cosechar: no se reusa. El grupo
                // (si es lider) tambien es ajeno.
                unsafe {
                    if grupo == pid {
                        libc::kill(-grupo, libc::SIGKILL);
                    }
                    libc::kill(pid, libc::SIGKILL);
                    if libc::waitpid(pid, std::ptr::null_mut(), 0) == pid {
                        esta_vuelta += 1;
                    }
                }
            }
            n += esta_vuelta;
            if esta_vuelta == 0 {
                break;
            }
        }
        n
    }

    pub(super) fn es_huerfano(pid: i32, grupo: i32, sesion: i32, mi_grupo: i32, mi_sesion: i32) -> bool {
        sesion != mi_sesion || (grupo != pid && grupo != mi_grupo)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux::es_huerfano;

    #[test]
    fn regla_de_huerfano() {
        let (g, s) = (100, 50);
        // node del runner o el hijo que minifica: lider de su grupo, nuestra sesion.
        assert!(!es_huerfano(7, 7, s, g, s));
        // node --check: nuestro grupo.
        assert!(!es_huerfano(8, g, s, g, s));
        // detached (setsid): otra sesion.
        assert!(es_huerfano(9, 9, 9, g, s));
        // resto del grupo de un node muerto.
        assert!(es_huerfano(10, 7, s, g, s));
    }

    /// Un hijo `detached` del programa no sobrevive a la ejecucion, y con
    /// `stdio: 'inherit'` no hace esperar READ_GRACE. Activa el subreaper en el
    /// proceso de tests: la regla no toca hijos legitimos de los otros tests.
    #[test]
    fn hijo_detached_no_sobrevive() {
        use crate::runner::{Preload, Runner, Status};
        use std::time::{Duration, Instant};
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        assert!(super::activar());
        let preload = Preload::write_tagged("subreaper").unwrap();
        let r = Runner::new(&preload, 9988, Duration::from_secs(20));
        let vivo = |pid: i32| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.starts_with(b"sleep\x0030"))
        };
        for stdio in ["ignore", "inherit"] {
            let js = format!(
                "const c = require('child_process').spawn('sleep', ['30'], {{ detached: true, stdio: '{stdio}' }});\nc.unref();\nconsole.log(c.pid);\n"
            );
            let t0 = Instant::now();
            let e = r.run(&js);
            assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
            let pid: i32 = e.stdout.trim().parse().unwrap();
            assert!(!vivo(pid), "stdio {stdio}: el sleep {pid} sigue vivo");
            assert!(t0.elapsed() < Duration::from_millis(900), "stdio {stdio}: tardo {:?}", t0.elapsed());
        }
    }
}
