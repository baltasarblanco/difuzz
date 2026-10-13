//! 7d: la clasificacion de una divergencia ya confirmada, en un solo lugar.
//!
//! `clasificar` decide si el caso va a `div/` o a `known/` (y por que), con
//! que flags y que notas, y deja la traza de cada paso (`Traza`), que el
//! reporter escribe al final de meta.txt en la seccion `clasificacion:`.
//!
//! Los pasos, en el orden de siempre (cada uno puede correr node o el
//! minifier: el orden de esas ejecuciones no cambia):
//! 1. contenido de toString/.stack (A con los contenidos cambiados; t-principio:
//!    y A'/B' con el contenido fijo),
//! 2. profundidad (M16, A con mas pila; t-principio: y B con mas pila; 8.1: un
//!    Explica se confirma con los literales salados; 9.2: con la sal amplia, que tiene que
//!    llegar a lo que B imprime distinto de A, y sin literales que salar es inconcluso),
//! 3. nombres (core_defaults: minificar preservando nombres),
//!    y 8.3: el nombre local en un mensaje de V8 (`mensaje_nombre`, si la politica lo pide),
//! 4. re-chequeos de la politica (`extends`, `global_conocido`, `with`,
//!    violaciones; t-principio: `builtins`, `coercion`),
//! 5. tabla de asunciones conocidas, rutas y backlog (known.rs),
//! 6. solo sloppy (M17: el original en estricto),
//! 7. minificado invalido (b1-b, R1-H03: nunca es asuncion),
//! 8. cambio de sintoma de una conocida (known-sintomas.tsv).
//!
//! Movido de main.rs (compare_and_report y sus re-chequeos) sin cambiar el
//! resultado: mismos destinos, flags y notas. Lo unico nuevo es la traza.

use std::path::Path;

use crate::backend::Rechequeo;
use crate::runner::Status;
use crate::{analisis, backend, minify, oracle, recheck, runner, same_exec, Ctx};

/// Flags cuya asuncion es sobre el contenido, no sobre la llamada.
pub const CONTENT_FLAGS: [&str; 2] = ["fn_tostring", "stack"];

/// 7a: el backend medido y su celda. Los re-chequeos propios del backend
/// (`backend::Politica`: `rechequeos`, `solo_sloppy`) re-minifican con esto.
#[derive(Clone, Copy, Debug)]
pub struct Medido {
    pub minifier: backend::Minifier,
    pub preset: minify::Preset,
    pub kind: minify::SourceKind,
}

impl Medido {
    fn rechequea(self, r: Rechequeo) -> bool {
        self.minifier.politica().rechequeos.contains(&r)
    }
}

/// 7d: lo que se intento al clasificar y lo que dio, un paso por linea.
#[derive(Debug, Default)]
pub struct Traza {
    pasos: Vec<(&'static str, String)>,
}

impl Traza {
    fn paso(&mut self, nombre: &'static str, resultado: impl Into<String>) {
        self.pasos.push((nombre, resultado.into()));
    }

    /// M1 (revision-b456): un paso previo a clasificar (hoy: A y B repetidos sin
    /// el sensor, main::compare_and_report), primero en la seccion.
    pub fn anteponer(&mut self, nombre: &'static str, resultado: impl Into<String>) {
        self.pasos.insert(0, (nombre, resultado.into()));
    }

    /// Un re-chequeo de asuncion (`recheck::Recheck`) y que hizo con el flag.
    fn recheck(&mut self, flag: &'static str, r: &recheck::Recheck, (irrelevante, inconcluso): (&str, &str)) {
        let t = match r {
            recheck::Recheck::Explica => format!("Explica (queda el flag {flag})"),
            recheck::Recheck::NoExplica => format!("NoExplica -> {irrelevante}"),
            recheck::Recheck::Inconcluso(n) => format!("Inconcluso -> {inconcluso}: {n}"),
        };
        self.paso(flag, t);
    }

    /// R2-H07: los flags que un re-chequeo confirmo como causa (`Explica`).
    /// El paso `contenido` confirma `fn_tostring` y `stack` a la vez; el de
    /// `violaciones` (b4-a), los flags de violacion que haya (`tdz_atrapada`,
    /// `coercion_symbol`, `extends_atrapado`: Explica deja todos los presentes;
    /// `Known::classify_causa` se queda con los que estan en los flags del caso).
    fn explicados(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for n in self.pasos.iter().filter(|(_, r)| r.starts_with("Explica")).map(|(n, _)| *n) {
            if n == "contenido" {
                v.extend_from_slice(&CONTENT_FLAGS);
            } else if n == "violaciones" {
                v.extend(recheck::FLAGS_VIOLACION.iter().map(|(f, _)| *f));
            } else {
                v.push(n);
            }
        }
        v
    }

    /// La seccion de meta.txt: `clasificacion:` y una linea por paso.
    pub fn texto(&self) -> String {
        let mut s = String::from("clasificacion:\n");
        for (n, r) in &self.pasos {
            s.push_str(&format!("  {n}: {r}\n"));
        }
        s
    }
}

/// El resultado de clasificar una divergencia: lo que va a meta.txt.
pub struct Clasificacion<'a> {
    /// Flags de A y estaticos, con los re-chequeos aplicados (`flags_a:`).
    pub flags: Vec<&'a str>,
    /// `Some(razon)`: `known/`. `None`: `div/`.
    pub why: Option<String>,
    /// `nota:` de meta.txt (re-chequeos inconclusos, analisis, sintoma nuevo).
    pub nota: Option<String>,
    /// Una conocida que cambio de sintoma (vuelve a `div/`).
    pub drift: bool,
    pub traza: Traza,
}

/// 10.1b: lo que dio el paso `contenido` (`paso_contenido`).
#[derive(Debug)]
pub enum PasoContenido {
    /// El original con otro contenido de toString/.stack (salado) da lo mismo: su salida no
    /// depende de ese contenido y la diferencia no se puede atribuir a el.
    Irrelevante,
    /// El original depende del contenido: lo que dio `recheck::contenido_matters` y, si fue
    /// `Explica` con la celda medida, como lo confirmo la sal de literales (si la sal no lo
    /// confirma, queda `Inconcluso` con su nota).
    Rechequeo(recheck::Recheck, Option<recheck::Salado>),
}

/// El paso `contenido` (`fn_tostring` y `stack`), el mismo para `clasificar` (A contra B,
/// `lados` = `Minificado`) y para `main::comparar_crudo` (10.1b, `--crudo`: el crudo hace de
/// original y A de minificado, `lados` = `Crudo`). Solo `Rechequeo(Explica, _)` atribuye la
/// diferencia al contenido.
/// - El original salado (`run_salted`): si da lo mismo, `Irrelevante`.
/// - t-principio: que el original dependa del contenido no dice que la divergencia venga de
///   ahi (a03/a04: un bug ajeno al lado iba a known/). A' y B' con el contenido fijo, y
///   `veredicto_neutralizado` con la cobertura (`recheck::contenido_matters`).
/// - b7-1b (revision 7.1, H2, opcion (c)): sin sensor de uso para el texto de una funcion del
///   programa (17-tostring-positivo lo usa), un Explica se confirma salado.
/// - 9.2 (M1): el `Salado` dice si la sal confirmo o si no habia literales que salar.
pub fn paso_contenido(
    runner: &runner::Runner,
    medido: Option<Medido>,
    lados: recheck::Lados,
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> PasoContenido {
    let salted = runner.run_salted(original);
    if salted.status == a.status && salted.stdout == a.stdout {
        return PasoContenido::Irrelevante;
    }
    match (recheck::contenido_matters(runner, original, minified, a, b, &salted), medido) {
        (recheck::Recheck::Explica, Some(m)) => {
            match recheck::con_literales_salados_de(runner, (m.minifier, m.preset, m.kind), original, recheck::Sal::Dato, lados, |o2, m2, a2, b2| {
                recheck::contenido_matters(runner, o2, m2, a2, b2, &runner.run_salted(o2))
            }) {
                Ok(s) => PasoContenido::Rechequeo(recheck::Recheck::Explica, Some(s)),
                Err(n) => PasoContenido::Rechequeo(recheck::Recheck::Inconcluso(n), None),
            }
        }
        (r, _) => PasoContenido::Rechequeo(r, None),
    }
}

/// Clasifica la divergencia `kind` entre `a` (original) y `b` (minificado).
/// Una asuncion habla del programa de entrada: clasifican los flags de A y los
/// estaticos del original. Un flag solo en B es algo que el minificado hace y
/// el original no (p. ej. el .stack que Node lee al cargar un SyntaxError):
/// hallazgo, no asuncion.
#[allow(clippy::too_many_arguments)]
pub fn clasificar<'a>(
    ctx: &Ctx,
    runner: &runner::Runner,
    rel: &Path,
    original: &str,
    minified: &str,
    a: &'a runner::Exec,
    b: &runner::Exec,
    kind: oracle::Kind,
    static_flags: &[&'static str],
    names_cfg: Option<&minify::Config>,
    nota_pasada: Option<&str>,
    // Integracion 7.1 + 7.4: el runner con el sensor si A y B se compararon sin el (I3); el
    // camino de B sale de ahi (`recheck::camino_de_b`).
    i3_con: Option<&runner::Runner>,
) -> Clasificacion<'a> {
    let mut traza = Traza::default();
    let mut flags: Vec<&str> = a.flags.iter().map(String::as_str).collect();
    flags.extend_from_slice(static_flags);
    // AGENTS.md habla de depender del *contenido* de toString y .stack, no
    // de llamarlos (`f++` llama a toString y da NaN igual). A se corre de
    // nuevo con esos contenidos cambiados: si la salida no cambia, no dependen.
    // b1-b: notas de los re-chequeos inconclusos (van a meta.txt).
    let mut notes: Vec<String> = Vec::new();
    if flags.iter().any(|f| CONTENT_FLAGS.contains(f)) {
        match paso_contenido(runner, ctx.medido, recheck::Lados::Minificado, original, minified, a, b) {
            PasoContenido::Irrelevante => {
                flags.retain(|f| !CONTENT_FLAGS.contains(f));
                flags.push("contenido_irrelevante");
                traza.paso("contenido", "NoExplica -> contenido_irrelevante (A con toString/.stack cambiados da lo mismo)");
            }
            PasoContenido::Rechequeo(r, salado) => match r {
                recheck::Recheck::Explica => traza.paso(
                    "contenido",
                    format!("Explica (A depende del contenido y con el contenido fijo la divergencia desaparece{})", con_sal(salado)),
                ),
                r => {
                    let t = match &r {
                        recheck::Recheck::Inconcluso(n) => format!("Inconcluso -> contenido_inconcluso: {n}"),
                        _ => "NoExplica -> contenido_irrelevante (con el contenido fijo la divergencia sigue)".into(),
                    };
                    traza.paso("contenido", t);
                    flags.retain(|f| !CONTENT_FLAGS.contains(f));
                    match r {
                        recheck::Recheck::Inconcluso(n) => {
                            notes.push(format!("re-chequeo de contenido (toString/.stack) inconcluso: {n}"));
                            flags.push("contenido_inconcluso");
                        }
                        _ => flags.push("contenido_irrelevante"),
                    }
                }
            }
        }
    }
    // M16: la salida puede depender del limite de pila de V8 y no del
    // programa (1000 funciones anidadas, una recursion que atrapa el
    // RangeError y devuelve cuantos frames entraron). Si A con el doble de
    // pila da otra salida, la divergencia depende de la profundidad del
    // motor. DESVIO de la propuesta: se prueba ante toda divergencia
    // confirmada, no solo si la salida dice "RangeError" (corpus-invocar/37
    // lo atrapa y no lo imprime). Cuesta una ejecucion por divergencia.
    // R1-H13/H14: la nota de la pasada (analisis con SWC, pasada de bloques).
    // t-principio: primera, como antes (las notas del contenido van despues).
    if let Some(n) = nota_pasada {
        notes.insert(0, n.to_string());
    }
    let deep = runner.run_with_args(original, &[runner::DEEP_STACK]);
    // b1-b (R2-H02): solo un M16 concluyente pone el flag (depth_recheck).
    let par = ("profundidad_irrelevante", "profundidad_inconcluso");
    let a_otra_vez = || runner.run(original);
    // b7-1 (r04: B todavia llega al limite de la pila y corre el catch con el bug): B con la
    // pila de siempre y con mas pila, con cobertura.
    let camino = |b2: &runner::Exec| recheck::camino_entorno(runner, minified, b, b2, (&[runner::DEEP_STACK], &[]), &[]);
    // 8.1 (rev-final F1: un desborde de pila NATIVO usado como dato, `JSON.stringify` de un
    // arreglo anidado; B recorre los mismos bloques con y sin mas pila y el camino no lo ve): sin
    // sensor de uso, un Explica se confirma salado, como contenido. El mismo M16 sobre el
    // original salado: si A salado ya no depende de la pila (o la corrida honda falla), no
    // confirma nada.
    // 9.2 (H2 de la revision del bloque 8: la tabla con `String("")`, sin nada que salar, o con
    // un literal salado que no es el de la coincidencia, seguia en known/): la sal es la amplia
    // (`Sal::Amplia`: tambien argumentos, ramas de `?:`, `return`...); sin literales que salar,
    // Inconcluso; y la sal tiene que llegar a las lineas en que B difiere de A
    // (`recheck::sal_alcanza`). Sin eso, que el salado de Explica no prueba nada.
    let mut salado = None;
    let r = match depth_recheck(a, b, &deep, a_otra_vez, || runner.run_with_args(minified, &[runner::DEEP_STACK]), camino) {
        Some(recheck::Recheck::Explica) => Some(match ctx.medido {
            Some(m) => {
                let c = recheck::con_literales_salados(runner, (m.minifier, m.preset, m.kind), original, recheck::Sal::Amplia, |o2, m2, a2, b2| {
                    if let Err(n) = recheck::sal_alcanza(a, b, b2) {
                        return recheck::Recheck::Inconcluso(n);
                    }
                    let deep2 = runner.run_with_args(o2, &[runner::DEEP_STACK]);
                    let camino2 = |b3: &runner::Exec| recheck::camino_entorno(runner, m2, b2, b3, (&[runner::DEEP_STACK], &[]), &[]);
                    depth_recheck(a2, b2, &deep2, || runner.run(o2), || runner.run_with_args(m2, &[runner::DEEP_STACK]), camino2)
                        .unwrap_or_else(|| {
                            recheck::Recheck::Inconcluso(
                                "A con mas pila da lo mismo que A (o la corrida honda fallo): la divergencia ya no depende de la pila".into(),
                            )
                        })
                });
                match c {
                    Ok(recheck::Salado::Confirmado) => {
                        salado = Some(recheck::Salado::Confirmado);
                        recheck::Recheck::Explica
                    }
                    Ok(recheck::Salado::SinLiterales) => recheck::Recheck::Inconcluso(
                        "no hay literales que salar (ni de dato ni de argumento, ramas de ?: o return): la coincidencia de B con A con mas pila no se puede confirmar con otro valor".into(),
                    ),
                    Err(n) => recheck::Recheck::Inconcluso(n),
                }
            }
            None => recheck::Recheck::Explica,
        }),
        otro => otro,
    };
    match r {
        None => traza.paso("profundidad", "no aplica (A con mas pila da lo mismo, o la corrida honda fallo)"),
        Some(recheck::Recheck::Explica) => {
            traza.paso("profundidad", format!("Explica (queda el flag profundidad{})", con_sal(salado)));
            flags.push("profundidad");
        }
        Some(r) => rechequear(&mut flags, &mut notes, &mut traza, "profundidad", par, r),
    }
    if let Some(c) = names_cfg {
        if !names_visible(original, &a.stdout) {
            traza.paso("nombres", "no aplica (la salida no puede mostrar nombres)");
        } else {
            match names_only(runner, c, (original, minified), a, b, ctx.medido) {
                (recheck::Recheck::Explica, detalle) => {
                    traza.paso("nombres", format!("Explica (con los nombres preservados la divergencia desaparece{detalle})"));
                    flags.push("nombres");
                }
                (recheck::Recheck::NoExplica, _) => traza.paso("nombres", "NoExplica (con los nombres preservados sigue)"),
                // 11.3: sin el flag `nombres` (div/), con la nota y `nombres_inconcluso`.
                (recheck::Recheck::Inconcluso(n), _) => {
                    traza.paso("nombres", format!("Inconcluso -> nombres_inconcluso: {n}"));
                    notes.push(format!("re-chequeo de nombres inconcluso: {n}"));
                    flags.push("nombres_inconcluso");
                }
            }
        }
    }
    // 8.3 (= 7.11; desde cd54ad2, d2/difuzz-1, hono sse.js): A y B difieren solo en mensajes de
    // V8 que escriben el nombre local de la expresion (`cb is not a function` / `e is not a
    // function`). Solo si la politica lo pide y la divergencia pasa la localizacion (si no, ni
    // corre ni deja traza). Explica exige que B sea un renombre correcto del minificado sin
    // mangle y que ese de lo mismo que A; si no se puede probar, div/ con nota.
    if let Some(m) = ctx.medido.filter(|m| m.rechequea(Rechequeo::MensajeNombre)) {
        if recheck::mensaje_con_nombre_localizado(a, b) {
            flags.push("mensaje_nombre");
            let r = recheck::mensaje_nombre_matters(runner, (m.minifier, m.preset, m.kind), original, minified, a, b);
            rechequear(&mut flags, &mut notes, &mut traza, "mensaje_nombre", ("mensaje_nombre_irrelevante", "mensaje_nombre_inconcluso"), r);
        }
    }
    // 7a: los re-chequeos de flags estaticos que declara la politica del
    // backend (oxc: los tres de M13), en el orden de siempre.
    let rc = ctx.medido.filter(|m| m.rechequea(Rechequeo::Extends));
    if flags.contains(&"extends") {
        if let Some(rc) = rc {
            let r = extends_matters(runner, rc, original, a, b, || recheck::camino_de_b(runner, i3_con, minified, b, Some(&["extends"])));
            rechequear(&mut flags, &mut notes, &mut traza, "extends", ("extends_irrelevante", "extends_inconcluso"), r);
        }
    }
    // Pendiente del fuego: `global_conocido` y `with` tambien se
    // re-chequean (recheck.rs), como `extends`.
    if let Some(rc) = ctx.medido {
        if flags.contains(&"global_conocido") && rc.rechequea(Rechequeo::GlobalConocido) {
            let r = recheck::globals_matter(runner, rc.kind, original, minified, a, b);
            rechequear(
                &mut flags,
                &mut notes,
                &mut traza,
                "global_conocido",
                ("global_conocido_irrelevante", "global_conocido_inconcluso"),
                r,
            );
        }
        if flags.contains(&"with") && rc.rechequea(Rechequeo::With) {
            let r = recheck::with_matters(runner, (rc.minifier, rc.preset, rc.kind), original, minified, a, b);
            rechequear(&mut flags, &mut notes, &mut traza, "with", ("with_irrelevante", "with_inconcluso"), r);
        }
        // b4-a (R1-H04 / C3): un re-chequeo para todas las violaciones ubicadas; su
        // resultado vale para cada flag de violacion presente (Explica los deja).
        let nombres: Vec<&'static str> =
            recheck::FLAGS_VIOLACION.iter().map(|(f, _)| *f).filter(|f| flags.contains(f)).collect();
        if !nombres.is_empty() && rc.rechequea(Rechequeo::Violaciones) {
            let r = recheck::violaciones_matter(runner, (rc.minifier, rc.preset, rc.kind), original, minified, a, b, i3_con);
            let t = match &r {
                recheck::Recheck::Explica => format!("Explica (quedan {})", nombres.join(", ")),
                recheck::Recheck::NoExplica => format!("NoExplica -> {}_irrelevante", nombres.join("_irrelevante, ")),
                recheck::Recheck::Inconcluso(n) => format!("Inconcluso -> {}_inconcluso: {n}", nombres.join("_inconcluso, ")),
            };
            traza.paso("violaciones", t);
            if let recheck::Recheck::Inconcluso(n) = &r {
                notes.push(format!("re-chequeo de violaciones ({}) inconcluso: {n}", nombres.join(", ")));
            }
            for f in nombres {
                let par = match f {
                    "tdz_atrapada" => ("tdz_atrapada_irrelevante", "tdz_atrapada_inconcluso"),
                    "coercion_symbol" => ("coercion_symbol_irrelevante", "coercion_symbol_inconcluso"),
                    // I2 (revision-b456): la TDZ no atrapada.
                    "tdz" => ("tdz_irrelevante", "tdz_inconcluso"),
                    _ => ("extends_atrapado_irrelevante", "extends_atrapado_inconcluso"),
                };
                // La nota ya va una vez arriba: aca solo el flag.
                let r1 = match &r {
                    recheck::Recheck::Inconcluso(_) => recheck::Recheck::Inconcluso(String::new()),
                    otro => otro.clone(),
                };
                let mut sin_nota = Vec::new();
                apply_recheck(&mut flags, &mut sin_nota, f, par, r1);
            }
        }
        // t-principio: `builtins` y `coercion` iban a known/ sin re-chequeo (a01, a02,
        // a08: un bug ajeno al lado).
        if flags.contains(&"builtins") && rc.rechequea(Rechequeo::Builtins) {
            let r = recheck::builtins_matter(runner, original, minified, a, b);
            rechequear(&mut flags, &mut notes, &mut traza, "builtins", ("builtins_irrelevante", "builtins_inconcluso"), r);
        }
        if flags.contains(&"coercion") && rc.rechequea(Rechequeo::Coercion) {
            let r = recheck::coercion_matters(runner, (rc.minifier, rc.preset, rc.kind), original, minified, a, b);
            rechequear(&mut flags, &mut notes, &mut traza, "coercion", ("coercion_irrelevante", "coercion_inconcluso"), r);
        }
    }
    // R2-H07: la razon es la del flag que un re-chequeo confirmo, si hay; si
    // no, la del primero de la tabla (como antes).
    let mut why = ctx.known.classify_causa(rel, &flags, &traza.explicados());
    traza.paso("asunciones", why.clone().unwrap_or_else(|| "ninguna (ni flag de la tabla, ni ruta, ni backlog)".into()));
    // M17: solo si la politica del backend lo pide y en el carril A (sloppy).
    let sloppy = ctx.medido.filter(|m| m.kind == minify::SourceKind::Script).and_then(|m| Some((m, m.minifier.politica().solo_sloppy?)));
    if let (None, Some((m, razon))) = (&why, sloppy) {
        // b7-1b (revision 7.1, H2, opcion (c)): el sensor de uso de sloppy solo ve el `this`; un
        // Explica por el estricto (`Si`, `SiRenombrando`) se confirma con los literales salados.
        // 9.2 (M1): la traza dice si la sal confirmo o si no habia literales que salar.
        let mut salado = None;
        let s = match sloppy_only(runner, (m.minifier, m.preset), original, minified, a, b) {
            Ok(si @ (Sloppy::Si | Sloppy::SiRenombrando)) => {
                let r = recheck::con_literales_salados(runner, (m.minifier, m.preset, m.kind), original, recheck::Sal::Dato, |o2, m2, a2, b2| {
                    match sloppy_only(runner, (m.minifier, m.preset), o2, m2, a2, b2) {
                        Ok(Sloppy::No) => recheck::Recheck::NoExplica,
                        Ok(_) => recheck::Recheck::Explica,
                        Err(n) => recheck::Recheck::Inconcluso(n),
                    }
                });
                match r {
                    Ok(c) => {
                        salado = Some(c);
                        Ok(si)
                    }
                    Err(n) => Err(format!("re-chequeo sloppy inconcluso: {n}")),
                }
            }
            otro => otro,
        };
        match s {
            Ok(Sloppy::Si) => {
                flags.push("solo_sloppy");
                why = Some(razon.into());
                traza.paso("solo_sloppy", format!("Explica (con \"use strict\" la divergencia desaparece{})", con_sal(salado)));
            }
            Ok(Sloppy::SiRenombrando) => {
                flags.push("solo_sloppy");
                why = Some(format!("{razon} (con el binding `arguments` renombrado: ligarlo solo es valido en sloppy)"));
                traza.paso(
                    "solo_sloppy",
                    format!(
                        "Explica (`arguments` ligado no compila en estricto; con el binding renombrado a {} y \"use strict\" la divergencia desaparece{})",
                        recheck::ARGUMENTS_RENOMBRADO,
                        con_sal(salado)
                    ),
                );
            }
            Ok(Sloppy::Renombrando) => {
                flags.push("solo_sloppy");
                why = Some(format!("{razon} (con el binding `arguments` renombrado: ligarlo solo es valido en sloppy)"));
                traza.paso(
                    "solo_sloppy",
                    format!(
                        "Explica (con el binding `arguments` renombrado a {}, sin \"use strict\", el original da lo mismo y la divergencia desaparece: ligar `arguments` solo es valido en sloppy)",
                        recheck::ARGUMENTS_RENOMBRADO
                    ),
                );
            }
            Ok(Sloppy::No) => traza.paso("solo_sloppy", "NoExplica (con \"use strict\" sigue)"),
            Err(nota) => {
                traza.paso("solo_sloppy", format!("Inconcluso: {nota}"));
                notes.push(nota)
            }
        }
    }
    // b1-b (R1-H03): un minificado que node no acepta nunca es asuncion:
    // ninguna permite que el minifier produzca codigo invalido. Se
    // clasifica sin flags (queda el backlog/ruta de SWC, que no son
    // asunciones). Solo se paga `node --check` si iba a known/.
    if why.is_some() {
        if runner.node_accepts(minified) {
            traza.paso("minificado_invalido", "no (node --check acepta el minificado)");
        } else if !runner.node_accepts(original) {
            traza.paso("minificado_invalido", "no (node --check rechaza tambien el original)");
        } else {
            let sin_flags = ctx.known.classify(rel, &[]);
            if sin_flags.is_none() {
                flags.push("minificado_invalido");
                notes.push("node --check rechaza el minificado (y acepta el original): ninguna asuncion lo explica".into());
            }
            traza.paso(
                "minificado_invalido",
                format!(
                    "si (node --check rechaza el minificado y acepta el original): sin flags, {}",
                    sin_flags.as_deref().unwrap_or("ninguna")
                ),
            );
            why = sin_flags;
        }
    }
    // Una conocida que cambia de sintoma deja de estar cubierta.
    let drift = why.as_ref().and_then(|w| {
        let h = oracle::symptom(kind, a, b);
        ctx.known
            .record_symptom(rel, h)
            .err()
            .map(|prev| format!("conocida ({w}) con sintoma nuevo: antes {prev:016x}, ahora {h:016x}"))
    });
    if drift.is_some() {
        why = None;
        traza.paso("sintoma", "cambio respecto de known-sintomas.tsv -> div/");
    } else if why.is_some() {
        traza.paso("sintoma", "el de known-sintomas.tsv (o primero registrado)");
    }
    // b1-b: el drift va primero (como antes, cuando pisaba la nota de sloppy).
    let drift_hubo = drift.is_some();
    if let Some(d) = drift {
        notes.insert(0, d);
    }
    let nota = (!notes.is_empty()).then(|| notes.join(" | "));
    match &why {
        Some(w) => traza.paso("decision", format!("known/ ({w})")),
        None => traza.paso("decision", "div/"),
    }
    sin_repetidos(&mut flags);
    Clasificacion { flags, why, nota, drift: drift_hubo, traza }
}

/// H07 (revision 2): un flag que llega dos veces (`extends` de A y de los
/// estaticos) va una sola vez en `flags_a:`, en el orden de su primera
/// aparicion. Solo cambia el texto: lo que clasifica (este modulo,
/// known::classify_causa) mira si un flag esta, no cuantas veces.
pub fn sin_repetidos(flags: &mut Vec<&str>) {
    let mut vistos = std::collections::HashSet::new();
    flags.retain(|f| vistos.insert(*f));
}

/// `apply_recheck` con su linea en la traza.
/// 9.2 (M1 de la revision del bloque 8): lo que la traza de un `Explica` salado agrega antes del
/// `)`. Vacio si no paso por la sal (sin backend medido, solo en los tests).
fn con_sal(s: Option<recheck::Salado>) -> String {
    s.map(|s| format!("; {}", s.traza())).unwrap_or_default()
}

fn rechequear(
    flags: &mut Vec<&str>,
    notes: &mut Vec<String>,
    traza: &mut Traza,
    flag: &'static str,
    par: (&'static str, &'static str),
    r: recheck::Recheck,
) {
    traza.recheck(flag, &r, par);
    apply_recheck(flags, notes, flag, par, r);
}

/// Precondicion de names_only: la salida del original puede mostrar nombres,
/// porque lee `.name` o imprime una instancia con el nombre de su clase
/// (`Foo { x: 7 }`). Sin esto, names_only tapaba bugs reales: keep_* tambien
/// apaga optimizaciones (inlining de funciones con nombre) y el 12188 dejaba de
/// dispararse.
fn names_visible(original: &str, a_stdout: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let reads_name = original
        .match_indices(".name")
        .any(|(i, _)| !original[i + 5..].chars().next().is_some_and(ident));
    let named_instance = a_stdout.lines().any(|l| {
        l.match_indices(" {")
            .any(|(i, _)| l[..i].chars().next_back().is_some_and(ident))
    });
    reads_name || named_instance
}

/// La divergencia desaparece si se minifica igual pero preservando nombres: es
/// el costo de keep_* apagado (`f.name`, `Foo { x: 7 }` impreso como `{ x: 7 }`),
/// no un bug. Se minifica el original ya impreso: es el mismo programa.
///
/// 11.3 (I6 de rev-integral-1, la regla de t-principio): con eso solo no alcanza. keep_names
/// tambien apaga pasadas de oxc (el inlining de una funcion o clase anonima de un solo uso, su
/// borrado si no se usa, el borrado del nombre de una expresion de funcion): un bug real en una de
/// ellas desaparece igual con keep_names y, en un programa que nombra `.name` en cualquier lado
/// (`names_visible` es textual), iba a known/. Ahora, ademas, B tiene que diferir de A solo en
/// nombres (`difiere_solo_en_nombres`): si no, Inconcluso (div/ con la nota). No hay paso de
/// camino como en `recheck::veredicto_neutralizado`: B y el minificado con keep_names son textos
/// distintos y la cobertura de V8 no se puede comparar entre ellos. Un minificado que nadie pudo
/// re-minificar con keep_names sigue siendo NoExplica, como antes.
///
/// Un valor que sale de un nombre (`inferred.name === 'inferred' ? f : null`, MEDIDO en el corpus
/// real: cjs/10506) no pasa `difiere_solo_en_nombres`. Para esos, si el backend tiene variante
/// sin mangle (oxc), la misma prueba que `recheck::mensaje_nombre_matters`: el minificado sin
/// mangle (B0) da lo mismo que A y B es un renombre de B0 (`analisis::alfa`, el resolver de SWC:
/// cada identificador resuelve al binding que corresponde, sin `with` ni `eval` directo). Entonces
/// B difiere de algo que se comporta como A solo en los nombres de sus bindings: Explica. Si el
/// nombre lo cambia una pasada de compress (12306 de SWC: oxc borra el nombre de una expresion de
/// funcion al inlinearla), B0 tampoco da lo mismo que A y sigue Inconcluso.
fn names_only(
    runner: &runner::Runner,
    cfg: &minify::Config,
    (original, minified): (&str, &str),
    a: &runner::Exec,
    b: &runner::Exec,
    medido: Option<Medido>,
) -> (recheck::Recheck, &'static str) {
    let minify::Outcome::Ok { minified: con_nombres, .. } = minify::prepare(original, cfg) else {
        return (recheck::Recheck::NoExplica, "");
    };
    if !matches!(oracle::compare(a, &runner.run(&con_nombres)), oracle::Verdict::Same) {
        return (recheck::Recheck::NoExplica, "");
    }
    let Err(n) = difiere_solo_en_nombres(a, b, original, minified, runner.kind()) else {
        return (recheck::Recheck::Explica, "");
    };
    let renombre = medido.map_or(Err("sin backend medido".to_string()), |m| solo_renombre(runner, m, original, minified, a));
    match renombre {
        Ok(()) => (recheck::Recheck::Explica, "; B es un renombre del minificado sin mangle, que da lo mismo que A"),
        Err(r) => (
            recheck::Recheck::Inconcluso(format!(
                "con los nombres preservados la divergencia desaparece, pero B difiere de A en algo que no es un nombre ({n}) y no se pudo ver que B sea un renombre de algo que se comporta como A ({r}): keep_names tambien apaga pasadas del minifier y el bug puede estar en una de ellas"
            )),
            "",
        ),
    }
}

/// 11.3: el minificado sin mangle (B0) da lo mismo que A y B es un renombre de B0 (ver
/// `names_only`). `Err` con la razon.
fn solo_renombre(runner: &runner::Runner, m: Medido, original: &str, minified: &str, a: &runner::Exec) -> Result<(), String> {
    let cfg = minify::Config::sin_mangle(m.minifier, m.preset, m.kind).ok_or_else(|| format!("{} no tiene variante sin mangle", m.minifier.name()))?;
    let minify::Outcome::Ok { minified: m0, .. } = minify::prepare(original, &cfg) else {
        return Err("no pude minificar sin mangle".into());
    };
    let b0 = runner.run(&m0);
    if !matches!(oracle::compare(a, &b0), oracle::Verdict::Same) {
        return Err("el minificado sin mangle tampoco da lo mismo que A".into());
    }
    match analisis::alfa(&m0, minified, m.kind) {
        Ok(Some(Ok(_))) => Ok(()),
        Ok(Some(Err(e))) => Err(format!("el minificado no es un renombre del minificado sin mangle: {e}")),
        Ok(None) => Err("el analisis de difuzz no parsea el minificado o el minificado sin mangle".into()),
        Err(e) => {
            analisis::registrar("re-chequeo de nombres (renombre)", &e);
            Err(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()))
        }
    }
}

/// Palabras que tienen forma de nombre pero son valores que un bug puede cambiar: los de
/// `String(x)` de los primitivos y lo que da `typeof`.
const NO_SON_NOMBRES: [&str; 13] =
    ["NaN", "Infinity", "undefined", "null", "true", "false", "number", "string", "object", "function", "boolean", "symbol", "bigint"];

/// 11.3: A y B (normalizados como en `oracle::compare`) terminan igual, imprimen la misma
/// cantidad de lineas y, en cada linea en que difieren, lo que cambia (por tokens de JS:
/// identificadores, numeros y signos sueltos, alineados con `recheck::lcs_cambiados`) son
/// nombres: identificadores que aparecen como identificador en el texto que corrio de ese lado
/// (`original` para A, `minified` para B) y que no son un valor (`NO_SON_NOMBRES`). Un numero, un
/// signo o una palabra de valor que cambia no es un nombre. `Err` con la razon.
pub(crate) fn difiere_solo_en_nombres(a: &runner::Exec, b: &runner::Exec, original: &str, minified: &str, kind: minify::SourceKind) -> Result<(), String> {
    if a.status != b.status || a.threw() != b.threw() {
        return Err(format!("A y B no terminan igual: {:?} contra {:?}", a.status, b.status));
    }
    let (na, nb) = oracle::normalizadas(a, b);
    let (la, lb): (Vec<&str>, Vec<&str>) = (na.lines().collect(), nb.lines().collect());
    if la.len() != lb.len() {
        return Err(format!("A imprime {} lineas y B {}", la.len(), lb.len()));
    }
    // Los identificadores del codigo (SWC, en el hijo de analisis), no las palabras de un string:
    // con `"PASS"`/`"FAIL"` en el fuente, un bug que imprime FAIL no pasa por un nombre.
    let ids = |s: &str| -> Result<std::collections::HashSet<String>, String> {
        match analisis::identificadores(s, kind) {
            Ok(Some(v)) => Ok(v.into_iter().collect()),
            Ok(None) => Err("el analisis de difuzz no parsea el original o el minificado".into()),
            Err(e) => {
                analisis::registrar("re-chequeo de nombres (identificadores)", &e);
                Err(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()))
            }
        }
    };
    let (mut ids_a, mut ids_b) = (None, None);
    for (i, (x, y)) in la.iter().zip(&lb).enumerate().filter(|(_, (x, y))| x != y) {
        let (tx, ty) = (tokens_js(x), tokens_js(y));
        let (Some(cx), Some(cy)) = (recheck::lcs_cambiados(&ty, &tx), recheck::lcs_cambiados(&tx, &ty)) else {
            return Err(format!("la linea {} es demasiado larga para alinear", i + 1));
        };
        // Los espacios que cambian no cuentan (`Foo { x: 7 }` contra `{ x: 7 }`: util.inspect no
        // pone el prefijo ni su espacio de una instancia sin nombre), pero una linea en que solo
        // cambian espacios no difiere en un nombre.
        let mut algun_nombre = false;
        for (lado, toks, cambiados, texto, cache) in [("A", &tx, &cx, original, &mut ids_a), ("B", &ty, &cy, minified, &mut ids_b)] {
            let conocidos: &std::collections::HashSet<String> = match cache {
                Some(c) => c,
                vacio => vacio.insert(ids(texto)?),
            };
            let mut distintos = toks.iter().zip(cambiados).filter(|(t, c)| **c && !t.trim().is_empty()).map(|(t, _)| t).peekable();
            algun_nombre |= distintos.peek().is_some();
            if let Some(t) = distintos.find(|t| !es_identificador(t) || NO_SON_NOMBRES.contains(&t.as_str()) || !conocidos.contains(t.as_str())) {
                return Err(format!("en la linea {} {lado} imprime `{t}`, que no es un nombre de su texto", i + 1));
            }
        }
        if !algun_nombre {
            return Err(format!("en la linea {} solo cambian espacios", i + 1));
        }
    }
    Ok(())
}

/// 11.3: los identificadores del codigo de `src` (nombres de bindings y referencias, de
/// propiedades y privados), sin repetir. `None` si SWC no lo parsea. Corre en el hijo de
/// analisis (`analisis::identificadores`).
pub(crate) fn identificadores_de(src: &str, kind: minify::SourceKind) -> Option<Vec<String>> {
    use swc_ecma_visit::{Visit, VisitWith};
    struct Ids(std::collections::BTreeSet<String>);
    impl Visit for Ids {
        fn visit_ident(&mut self, n: &swc_ecma_ast::Ident) {
            self.0.insert(n.sym.to_string());
        }
        fn visit_ident_name(&mut self, n: &swc_ecma_ast::IdentName) {
            self.0.insert(n.sym.to_string());
        }
        fn visit_private_name(&mut self, n: &swc_ecma_ast::PrivateName) {
            self.0.insert(n.name.to_string());
        }
    }
    swc_common::GLOBALS.set(&swc_common::Globals::new(), || {
        let p = crate::backend::swc::parse_for(src, kind)?;
        let mut v = Ids(Default::default());
        p.visit_with(&mut v);
        Some(v.0.into_iter().collect())
    })
}

fn es_identificador(t: &str) -> bool {
    t.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
}

/// Tokens de JS para `difiere_solo_en_nombres`: identificadores (`[letra_$][letra digito _$]*`),
/// numeros (digitos seguidos) y cualquier otro caracter suelto.
fn tokens_js(s: &str) -> Vec<String> {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let (mut v, mut it) = (Vec::new(), s.chars().peekable());
    while let Some(c) = it.next() {
        let mut t = String::from(c);
        if c.is_alphabetic() || c == '_' || c == '$' {
            while let Some(&d) = it.peek().filter(|d| ident(**d)) {
                t.push(d);
                it.next();
            }
        } else if c.is_ascii_digit() {
            while let Some(&d) = it.peek().filter(|d| d.is_ascii_digit()) {
                t.push(d);
                it.next();
            }
        }
        v.push(t);
    }
    v
}

/// M13: el flag `extends` (estatico) mira todo el programa: en 9460 un
/// `class extends 1` atrapado a proposito tapaba un bug real (O5). Se cambia
/// cada heredero literal por `null`, se minifica igual y se compara: si la
/// divergencia sigue, `extends` no la explica (`false`). Si no hay nada que
/// cambiar o algo falla, inconcluso (b1-b: antes `true`, se conservaba el flag).
/// El veredicto es el de recheck::veredicto (camino incluido).
/// n-principio (n01/n02/n04: `extends 1` atrapado y el bug en el catch, que con
/// `extends null` no corre): rige `recheck::veredicto_neutralizado` (B == A').
/// b7-1: con `camino` (el sensor no tiene que ver en B un `extends` que lanza; si el bug deja
/// el valor por defecto, B == A' es coincidencia).
fn extends_matters(
    runner: &runner::Runner,
    recheck: Medido,
    original: &str,
    a: &runner::Exec,
    b: &runner::Exec,
    camino: impl FnOnce() -> recheck::Camino,
) -> recheck::Recheck {
    let Medido { minifier, preset, kind } = recheck;
    let neutral = match analisis::sin_extends(original, kind) {
        Ok(n) => n,
        Err(e) => {
            analisis::registrar("re-chequeo de extends", &e);
            return recheck::Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let Some(neutral) = neutral else {
        return recheck::Recheck::Inconcluso("no pude neutralizar el heredero literal".into());
    };
    let cfg = preset.build(minifier, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return recheck::Recheck::Inconcluso("no pude minificar la variante con `extends null`".into());
    };
    let a2 = runner.run(&o2);
    if !a2.completed() {
        return recheck::Recheck::Inconcluso(format!("la variante con `extends null` no termino ({:?})", a2.status));
    }
    let b2 = runner.run(&m2);
    recheck::veredicto_neutralizado(a, b, &a2, &b2, None, camino)
}

/// b1-b: aplica a `flags` el resultado del re-chequeo de `flag`. Solo `Explica`
/// lo deja; `NoExplica` lo cambia por `irrelevante` y `Inconcluso` por
/// `inconcluso` mas una nota para meta.txt.
fn apply_recheck(
    flags: &mut Vec<&str>,
    notes: &mut Vec<String>,
    flag: &str,
    (irrelevante, inconcluso): (&'static str, &'static str),
    r: recheck::Recheck,
) {
    let reemplazo = match r {
        recheck::Recheck::Explica => return,
        recheck::Recheck::NoExplica => irrelevante,
        recheck::Recheck::Inconcluso(n) => {
            notes.push(format!("re-chequeo de {flag} inconcluso: {n}"));
            inconcluso
        }
    };
    flags.retain(|f| *f != flag);
    flags.push(reemplazo);
}

/// b1-b (R2-H02, R1-H03): M16 concluyente. `None` si A no depende de la pila (o
/// la corrida honda fallo por el fuzzer). La profundidad explica la divergencia
/// solo si la corrida con mas pila termino sola y B difiere de A unicamente en
/// lo que tambien cambia con mas pila: B da lo mismo que A con mas pila, o las
/// tres salidas tienen las mismas lineas y cada linea en que B difiere de A es
/// una que cambia con la pila. Antes un Timeout de la corrida honda (o un bug en
/// otra linea) alcanzaba para mandar a known/.
/// t-principio (a07-profundidad-linea): un bug ajeno en la MISMA linea que la cuenta
/// de pila pasaba `localizada` (la linea cambia con mas pila) y el caso iba a known/.
/// Ahora rige `recheck::veredicto` con A' = A con mas pila y B' = B con mas pila
/// (`deep_b`, solo se corre si A depende de la pila): la divergencia tiene que
/// desaparecer con mas pila. Una salida que cuenta marcos sin tope (A' != B' por el
/// tamano de los marcos) ya no es concluyente: NoExplica, div/.
/// Paso 7 (revision-tint B1, r09: el bug en el catch del RangeError, que con mas pila no
/// corre): rige `recheck::veredicto_neutralizado`, sin `localizada`: B tiene que dar lo
/// mismo que A con mas pila.
/// n-ruido (t-real: Timeout de node con `expiry` de reloj real): que A con mas pila de
/// otra salida no prueba que A dependa de la pila si A tal cual tampoco se repite. Antes
/// de atribuir a la pila se corre A otra vez sin cambiar nada (`a_otra_vez`, solo si la
/// corrida honda difiere): si difiere de A, la causa no es la pila y el re-chequeo es
/// Inconcluso (no clasifica; el destino lo deciden los demas). Una cuenta de pila
/// determinista no cambia de camino.
/// b7-1: `camino` recibe B con mas pila (B') y dice si B cambia de camino con mas pila
/// (`recheck::veredicto_neutralizado`).
fn depth_recheck(
    a: &runner::Exec,
    b: &runner::Exec,
    deep: &runner::Exec,
    a_otra_vez: impl FnOnce() -> runner::Exec,
    deep_b: impl FnOnce() -> runner::Exec,
    camino: impl FnOnce(&runner::Exec) -> recheck::Camino,
) -> Option<recheck::Recheck> {
    if matches!(deep.status, Status::Failed(_)) || same_exec(deep, a) {
        return None;
    }
    if !deep.completed() {
        return Some(recheck::Recheck::Inconcluso(format!(
            "la corrida con mas pila ({}) no termino ({:?})",
            runner::DEEP_STACK,
            deep.status
        )));
    }
    let otra = a_otra_vez();
    if !matches!(otra.status, Status::Failed(_)) && !same_exec(&otra, a) {
        return Some(recheck::Recheck::Inconcluso(
            "A tal cual, sin cambiar la pila, tampoco da lo mismo: la salida no es determinista (reloj real u otra fuente) y la diferencia con mas pila no se puede atribuir a la pila".into(),
        ));
    }
    let b2 = deep_b();
    Some(recheck::veredicto_neutralizado(a, b, deep, &b2, None, || camino(&b2)))
}

/// M17: oxc no soporta sloppy (D1 del CLAUDE.md). Ante una divergencia de oxc
/// en el carril A: si el original con `"use strict"` es valido para node y la
/// divergencia desaparece al minificarlo asi, es "solo sloppy" (`Ok(true)`). Si
/// no es estricto-valido, `Err(nota)`: queda en div/ para confirmar en el
/// carril B. Se minifica el original ya impreso: es el mismo programa.
/// b1-b (R1-H01): ademas el camino tiene que sostenerse (recheck::veredicto). Si
/// en estricto el original toma otra rama, la divergencia puede "desaparecer"
/// porque la rama con el bug no corre: eso es inconcluso (`Err`), no sloppy.
/// R2-H10 (terser collapse_vars_arguments): si el original liga `arguments`
/// (`var arguments = 5`), con "use strict" no compila. Antes eso cortaba el
/// re-chequeo; ahora se prueba la variante estricta con ese binding renombrado
/// (`recheck::rename_arguments`) y rige el mismo `veredicto`: solo si la
/// divergencia desaparece y el camino se sostiene es "solo sloppy"
/// (`SiRenombrando`). Si renombrar no alcanza para que compile, sigue `Err`.
/// B1 (revision-b456): ademas el renombre solo (en sloppy) tiene que dar lo mismo
/// que A; si no, `Err` (el renombre cambio el programa, p. ej. `var arguments;`).
/// n-principio: rige `recheck::veredicto_neutralizado` (B == A'): con `veredicto`, un
/// bug en la rama que solo corre en sloppy (n33: `f() === undefined ? "estricto" :
/// Math.cbrt(-8)`; n34: A' == A por construccion) iba a known/. Para el binding
/// `arguments` se prueba antes el renombre SOLO, en sloppy y minificado igual
/// (`Sloppy::Renombrando`): A con el binding renombrado ya da lo mismo que A (B1), asi
/// que el renombre no cambia ningun camino; si ademas el minificado del renombrado da
/// lo mismo que A, lo unico que separa a B de un minificado correcto es el NOMBRE del
/// binding, y ligar `arguments` solo es valido en sloppy (collapse_vars_arguments: oxc
/// confunde el `arguments` propio de `inner` con el binding de afuera). Un bug que no
/// depende del nombre sigue en el minificado del renombrado (NoExplica por ese lado) y
/// cae al estricto con B == A'.
/// b7-1 (q05/q12: la rama que solo corre en sloppy, con el bug, y el valor por defecto en
/// B == A'): el estricto ademas exige que B no cambie de camino en estricto
/// (`recheck::camino_estricto`). `Renombrando` no compara contra A', no cambia.
fn sloppy_only(
    runner: &runner::Runner,
    (minifier, preset): (backend::Minifier, minify::Preset),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Result<Sloppy, String> {
    let mut strict = format!("\"use strict\";\n{original}");
    let mut si = Sloppy::Si;
    if !runner.node_accepts(&strict) {
        let renombrado = match analisis::sin_arguments(original) {
            Ok(r) => r,
            Err(e) => {
                analisis::registrar("re-chequeo sloppy (renombrar `arguments`)", &e);
                None
            }
        };
        let variante = renombrado.as_ref().map(|r| format!("\"use strict\";\n{r}"));
        match variante {
            Some(v) if runner.node_accepts(&v) => {
                // B1 (revision-b456, r01-args-getter): el renombre tiene que ser neutro.
                // `var arguments;` sin inicializador conserva el objeto `arguments` en
                // sloppy; renombrado vale `undefined`, A' lanza en la linea del bug y la
                // tapa. Se corre el renombrado SIN "use strict": si no da lo mismo que A,
                // renombrar cambio el programa y el re-chequeo no prueba nada (inconcluso).
                // Asi el veredicto de abajo solo mide lo que cambia el estricto.
                let ren = renombrado.as_deref().unwrap_or_default();
                let a_ren = runner.run(ren);
                if !crate::same_exec(&a_ren, a) {
                    return Err(format!(
                        "re-chequeo sloppy inconcluso: con el binding `arguments` renombrado a {} (sin \"use strict\") el original ya cambia ({:?})",
                        recheck::ARGUMENTS_RENOMBRADO,
                        a_ren.status
                    ));
                }
                // n-principio: el renombre solo, minificado igual (ver el comentario de la funcion).
                let cfg = preset.build(minifier, minify::SourceKind::Script);
                if let minify::Outcome::Ok { minified: m_ren, .. } = minify::prepare(ren, &cfg) {
                    let b_ren = runner.run(&m_ren);
                    if matches!(oracle::compare(a, &b_ren), oracle::Verdict::Same) {
                        return Ok(Sloppy::Renombrando);
                    }
                }
                strict = v;
                si = Sloppy::SiRenombrando;
            }
            _ => return Err(rechazo_estricto(runner, &strict)),
        }
    }
    let cfg = preset.build(minifier, minify::SourceKind::Script);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&strict, &cfg) else {
        return Err("re-chequeo sloppy inconcluso: no pude minificar la variante estricta".into());
    };
    let a2 = runner.run(&o2);
    if !a2.completed() {
        return Err(format!("re-chequeo sloppy inconcluso: la variante estricta no termino ({:?})", a2.status));
    }
    let b2 = runner.run(&m2);
    match recheck::veredicto_neutralizado(a, b, &a2, &b2, None, || recheck::camino_estricto(runner, minified, b)) {
        recheck::Recheck::Explica => Ok(si),
        recheck::Recheck::NoExplica => Ok(Sloppy::No),
        recheck::Recheck::Inconcluso(n) => Err(format!("re-chequeo sloppy inconcluso: {n}")),
    }
}

/// M17 / R2-H10: resultado concluyente del re-chequeo sloppy.
#[derive(Debug, PartialEq, Eq)]
enum Sloppy {
    /// Con "use strict" la divergencia sigue.
    No,
    /// Con "use strict" la divergencia desaparece.
    Si,
    /// Idem, con el binding `arguments` renombrado (si no, no compilaba).
    SiRenombrando,
    /// n-principio: con el binding `arguments` renombrado (sin "use strict") la
    /// divergencia ya desaparece: la explica ligar `arguments`, valido solo en sloppy.
    Renombrando,
}

/// R2-H12: la nota cuando node rechaza la variante estricta. `node --check` no
/// dice por que; "no es estricto-valido" solo si es un SyntaxError. Un
/// RangeError es V8 que no puede compilar el fuente (anidamiento extremo), no
/// sintaxis. Se corre la variante: no compila, asi que no ejecuta nada del
/// programa; la primera linea del preload dice el tipo (como main.rs con el
/// original invalido, R2-H03).
fn rechazo_estricto(runner: &runner::Runner, strict: &str) -> String {
    let e = runner.run(strict);
    let primera = e.stdout.lines().next().unwrap_or_default();
    if primera.starts_with(&format!("{}throw SyntaxError", runner::DZ)) {
        "no es estricto-valido: confirmar en el carril B".into()
    } else if primera.starts_with(&format!("{}throw RangeError", runner::DZ)) {
        "re-chequeo sloppy inconcluso: V8 no compila la variante estricta (RangeError, no es sintaxis)".into()
    } else {
        format!(
            "re-chequeo sloppy inconcluso: node --check rechaza la variante estricta sin SyntaxError ({:?}, {:?})",
            e.status, primera
        )
    }
}

#[cfg(test)]
mod tests {
    use super::names_visible;

    /// H07 (revision 2): sin repetidos, en el orden de la primera aparicion.
    #[test]
    fn flags_sin_repetidos_en_orden() {
        let mut f = vec!["extends", "coercion", "extends", "with", "coercion", "extends"];
        super::sin_repetidos(&mut f);
        assert_eq!(f, ["extends", "coercion", "with"]);
        let mut v: Vec<&str> = Vec::new();
        super::sin_repetidos(&mut v);
        assert!(v.is_empty());
    }

    /// 11.3 (I6 de rev-integral-1): el re-chequeo de nombres solo explica una divergencia en que
    /// B difiere de A en nombres de su texto. El bug E de oxc (`-2` contra `NaN`), un `typeof` que
    /// cambia o un nombre que no esta en el minificado no son nombres.
    #[test]
    fn difiere_solo_en_nombres_pide_nombres() {
        use crate::runner::{Exec, Status};
        let ex = |status: Status, out: &str| Exec {
            status,
            stdout: out.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let ok = |out: &str| ex(Status::Exited(0), out);
        let solo = |a: &Exec, b: &Exec, o: &str, m: &str| super::difiere_solo_en_nombres(a, b, o, m, crate::minify::SourceKind::Script);
        let (o, m) = ("(function () { function handler() {} class Foo {} console.log(handler.name, Object.name); })();", "(function(){function e(){}class t{}console.log(e.name,Object.name)})();");
        // Nombres: `.name`, la clase de una instancia, un mensaje de V8 y un stack.
        assert_eq!(solo(&ok("handler Object\n"), &ok("e Object\n"), o, m), Ok(()));
        assert_eq!(solo(&ok("Foo { x: 7 }\n"), &ok("t { x: 7 }\n"), o, m), Ok(()));
        assert_eq!(solo(&ok("1\nhandler is not a function\n"), &ok("1\ne is not a function\n"), o, m), Ok(()));
        // Un `.name` que en B queda vacio, y una instancia de una clase que en B no tiene nombre
        // (sin el prefijo ni su espacio: negate_iife_issue_1073 de terser).
        assert_eq!(solo(&ok("name: handler\n"), &ok("name: \n"), o, m), Ok(()));
        assert_eq!(solo(&ok("Foo { x: 7 }\n"), &ok("{ x: 7 }\n"), o, m), Ok(()));
        // Solo espacios no es un nombre.
        assert!(solo(&ok("a b\n"), &ok("a  b\n"), o, m).is_err(), "solo espacios");
        // Un valor que sale de un nombre (`f.name === 'g'`, fixture 12306 de SWC) no se distingue
        // de un bug que cambia un booleano (la identidad de una funcion inlineada): no es un nombre.
        assert!(solo(&ok("true\n"), &ok("false\n"), "f.name === 'g'", "f.name==='g'").is_err());
        // No son nombres.
        assert!(solo(&ok("Object -2\n"), &ok("Object NaN\n"), o, m).is_err(), "el bug E de oxc");
        assert!(solo(&ok("handler -2\n"), &ok("e NaN\n"), o, m).is_err(), "un nombre y el bug en la misma linea");
        assert!(solo(&ok("number\n"), &ok("string\n"), "typeof x === 'number' || 'string'", "typeof x=='number'||'string'").is_err());
        assert!(solo(&ok("handler 1\n"), &ok("e 2\n"), o, m).is_err(), "un numero que cambia");
        assert!(solo(&ok("handler\n"), &ok("otro\n"), o, m).is_err(), "`otro` no esta en el minificado");
        assert!(solo(&ok("handler\n"), &ok("e\nhandler\n"), o, m).is_err(), "otra cantidad de lineas");
        assert!(solo(&ok("handler\n"), &ex(Status::Exited(1), "e\n"), o, m).is_err(), "otro estado");
        // Las palabras de un string no son nombres del codigo: un bug que imprime FAIL en vez de
        // PASS en un programa que nombra `.name` no pasa (los identificadores salen de SWC).
        let (o2, m2) = ("console.log(f.name, ok ? 'PASS' : 'FAIL');", "console.log(f.name,ok?'PASS':'FAIL');");
        assert!(solo(&ok("f PASS\n"), &ok("f FAIL\n"), o2, m2).is_err(), "PASS/FAIL");
        // Un texto que SWC no parsea: no se puede saber.
        assert!(solo(&ok("handler\n"), &ok("e\n"), "var = ;", m).is_err(), "no parsea");
    }

    #[test]
    fn nombres_visibles_solo_si_la_salida_puede_mostrarlos() {
        assert!(names_visible("console.log(f.name);", ""));
        assert!(names_visible("", "Foo { x: 7 }"));
        assert!(names_visible("", "A {} A {}"));
        // 12188: nada de nombres a la vista.
        assert!(!names_visible("a[index++]=1;", "member result 1 1"));
        assert!(!names_visible("o.names;", "x = { a: 1 }"));
        assert!(!names_visible("", "Map(1) { 'a' => 1 }"));
    }

    /// R2-H07 con los flags de b4-a: el paso `violaciones` con Explica confirma los
    /// flags de violacion presentes, y la razon es la de ese flag aunque otro de la
    /// tabla (`coercion`, sin re-chequeo) venga antes.
    #[test]
    fn explicados_incluye_violaciones() {
        use crate::{backend::Minifier, known::Known};
        use std::path::Path;
        let mut t = super::Traza::default();
        t.paso("violaciones", "Explica (quedan tdz_atrapada)");
        t.paso("with", "NoExplica -> with_irrelevante");
        let e = t.explicados();
        for (f, _) in crate::recheck::FLAGS_VIOLACION {
            assert!(e.contains(&f), "{e:?}");
        }
        assert!(!e.contains(&"with") && !e.contains(&"violaciones"), "{e:?}");
        let dir = std::env::temp_dir().join(format!("difuzz-test-explicados-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let k = Known::load(&dir, &dir, 0, Minifier::Oxc);
        std::fs::remove_dir_all(&dir).unwrap();
        let p = Path::new("x/input.js");
        let flags = ["coercion", "tdz_atrapada"];
        assert!(k.classify(p, &flags).unwrap().contains("Coercion Methods Are Pure"));
        let causa = k.classify_causa(p, &flags, &e).unwrap();
        assert!(causa.contains("oxc: No TDZ Violation (atrapada)"), "{causa}");
    }

    /// M17: "solo sloppy" exige que el original sea estricto-valido y que la
    /// divergencia desaparezca en estricto. Un bug real (O1) no se esconde.
    #[test]
    fn oxc_solo_sloppy() {
        use super::sloppy_only;
        use crate::{backend::Minifier, minify, minify::Preset, runner};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = runner::Preload::write_tagged("m17").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        // b1-b: A y B de verdad (oxc core_defaults), no solo la variante estricta.
        let ab = |src: &str| {
            let cfg = Preset::CoreDefaults.build(Minifier::Oxc, minify::SourceKind::Script);
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            (original, minified, a, b)
        };
        // terser/compress/dead_code/dead_code_2_should_warn (corpus completo): `g`
        // es una funcion en bloque (Anexo B, solo sloppy). En sloppy `g()` da
        // TypeError, en estricto ReferenceError: mismo punto. Se aceptaba como sloppy
        // (`mismo_camino`). n-principio: B (`throw Error`) no es A' (`throw ReferenceError`)
        // y con la misma forma pasaba n34 (A' == A, el bug en la rama sloppy): inconcluso.
        let (o, m, a, b) = ab("function f(){g();x=10;throw new Error(`foo`);if(x){y();var x;function g(){}(function(){var q;function y(){}})()}}f();\n");
        assert_eq!(a.stdout, "!dz throw TypeError\n");
        let s = sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("B != A'")), "{s:?}");
        // n-principio (n33, n34): el bug en la rama que solo corre en sloppy. Con `veredicto`
        // iban a known/ (`localizada` y `mismo_camino`); ahora inconcluso.
        for src in [
            "function f() { return this; }\nvar r = f() === undefined ? \"estricto\" : Math.cbrt(-8);\nconsole.log(r);\n",
            "function f() { return this; }\nvar r = f() === undefined ? \"s\" : (Math.cbrt(-8) < 0 ? \"s\" : \"x\");\nconsole.log(r);\n",
        ] {
            let (o, m, a, b) = ab(src);
            assert_ne!(b.stdout, a.stdout, "oxc ya no pliega mal Math.cbrt(-8): buscar otro bug");
            let s = sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b);
            assert!(s.as_ref().is_err_and(|n| n.contains("B != A'")), "{src}: {s:?}");
        }
        let with = "with ({}) { console.log(1); }\n";
        assert!(sloppy_only(&r, (Minifier::Oxc, Preset::CarrilA), with, with, &r.run(with), &r.run(with)).is_err());
        let (o, m, a, b) = ab("const __proto__ = 0;\nconsole.log(Object.keys({ __proto__ }));\n");
        assert_eq!(sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b), Ok(super::Sloppy::No));
        // b1-b (R1-H01, c4 10-rt-sloppy-miente): el bug de oxc (borra la IIFE con
        // rest y default con efecto) esta en la rama sloppy. En estricto el
        // original toma la otra rama: antes `Ok(true)` (known/), ahora inconcluso.
        let (o, m, a, b) = ab("function rec(v) { console.log(v); }\nfunction f() { return this; }\nif (f() === undefined) {\n  console.log(\"estricto\");\n} else {\n  (function (...[value = rec(\"rest\")]) {})();\n}\nconsole.log(\"fin\");\n");
        assert_eq!(a.stdout, "rest\nfin\n");
        assert_ne!(b.stdout, a.stdout, "oxc ya no tiene el bug de 9460: buscar otro");
        let s = sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("B != A'")), "{s:?}");
    }

    /// b7-1b (revision 7.1, H1/H2), con node y oxc: la rama sloppy con el bug deja el valor por
    /// defecto y B elige ese valor por el `this` de sloppy (d07: el sensor de uso), por un `&&=`
    /// que oxc arma de un `if` (los contadores de rama) o por el `arguments` atado como dato (la
    /// opcion (c): con los literales salados B != A'). Antes, Explica.
    #[test]
    fn solo_sloppy_dependencia_por_datos() {
        use super::{sloppy_only, Sloppy};
        use crate::{backend::Minifier, minify, minify::Preset, recheck, runner};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = runner::Preload::write_tagged("b71b-sloppy").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let rc = (Minifier::Oxc, Preset::CarrilA);
        let ab = |src: &str| {
            let cfg = Preset::CarrilA.build(Minifier::Oxc, minify::SourceKind::Script);
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("\"bc\"\n", "\"\"\n"), "oxc ya no pliega mal substring: buscar otro bug\n{minified}");
            (original, minified, a, b)
        };
        let bug = "\"abc\".substring(1, undefined)";
        let (o, m, a, b) = ab(&format!("var t = [\"\", {bug}];\nconsole.log(JSON.stringify(t[+((function () {{ return this; }})() !== undefined)]));\n"));
        let s = sloppy_only(&r, rc, &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("B usa la asuncion: sloppy this")), "d07: {s:?}");
        let (o, m, a, b) = ab(&format!("function al(a) {{ arguments[0] = 2; return a; }}\nlet s = \"\";\nlet u = al(1) === 2;\nif (u) u = (s = {bug});\nconsole.log(JSON.stringify(s));\n"));
        assert!(m.contains("&&="), "oxc ya no arma `&&=`: {m}");
        let s = sloppy_only(&r, rc, &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("cobertura de bloques")), "sloppy-args-andasig: {s:?}");
        // El `arguments` atado como dato: ni el camino ni el sensor lo ven (Si); con la sal, no.
        let (o, m, a, b) = ab(&format!("function al(a) {{ arguments[0] = 2; return a; }}\nconst t = [\"\", {bug}];\nconsole.log(JSON.stringify(t[+(al(1) === 2)]));\n"));
        assert_eq!(sloppy_only(&r, rc, &o, &m, &a, &b), Ok(Sloppy::Si), "sloppy-args-tabla sin la sal");
        let c = recheck::con_literales_salados(&r, (rc.0, rc.1, minify::SourceKind::Script), &o, recheck::Sal::Dato, |o2, m2, a2, b2| {
            match sloppy_only(&r, rc, o2, m2, a2, b2) {
                Ok(Sloppy::No) => recheck::Recheck::NoExplica,
                Ok(_) => recheck::Recheck::Explica,
                Err(n) => recheck::Recheck::Inconcluso(n),
            }
        });
        assert!(matches!(c, Err(ref n) if n.contains("literales salados")), "sloppy-args-tabla salado: {c:?}");
    }

    /// R2-H10: un original que liga `arguments` no compila en estricto. El
    /// re-chequeo sloppy lo prueba con el binding renombrado y el mismo
    /// veredicto: el caso del corpus pasa a "solo sloppy"; un bug real al lado
    /// (o en la rama que el estricto apaga) no.
    #[test]
    fn solo_sloppy_con_arguments_ligado() {
        use super::{sloppy_only, Sloppy};
        use crate::{backend::Minifier, minify, minify::Preset, runner};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = runner::Preload::write_tagged("r2h10").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let ab = |src: &str, preset: Preset| {
            let cfg = preset.build(Minifier::Oxc, minify::SourceKind::Script);
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            (original, minified, a, b)
        };
        // terser/compress/collapse_vars/collapse_vars_arguments (corpus completo).
        let cva = "var outer = function () {\n    var k = 7,\n        arguments = 5,\n        inner = function () {\n            console.log(arguments);\n        };\n    inner(k, 1);\n};\nouter();\n";
        for preset in [Preset::CoreDefaults, Preset::CarrilA] {
            let (o, m, a, b) = ab(cva, preset);
            assert_eq!(a.stdout, "[Arguments] { '0': 7, '1': 1 }\n");
            assert_ne!(b.stdout, a.stdout, "oxc ya no tiene el bug de collapse_vars_arguments: buscar otro");
            // n-principio: con el renombre solo (sin "use strict") el minificado ya da lo mismo.
            // Antes SiRenombrando (estricto + `mismo_camino`, que ya no rige).
            assert_eq!(sloppy_only(&r, (Minifier::Oxc, preset), &o, &m, &a, &b), Ok(Sloppy::Renombrando), "{preset:?}");
        }
        // Miente: `var arguments` inofensivo y el bug de 9460 (IIFE con rest y
        // default con efecto) en otra sentencia. Renombrado y en estricto, el bug sigue.
        let (o, m, a, b) = ab("function rec(v) { console.log(v); }\nfunction g() { var arguments = 1; return arguments; }\nconsole.log(g());\n(function (...[value = rec(\"rest\")]) {})();\nconsole.log(\"fin\");\n", Preset::CoreDefaults);
        assert_eq!(a.stdout, "1\nrest\nfin\n");
        assert_ne!(b.stdout, a.stdout, "oxc ya no tiene el bug de 9460: buscar otro");
        assert_eq!(sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b), Ok(Sloppy::No));
        // B1 (revision-b456, r01-args-getter): `var arguments;` sin inicializador conserva
        // el objeto `arguments`; renombrado vale `undefined` y A' lanza en la linea del
        // bug (Issue A de ENVIAR.md: el spread con getter se mueve). Antes: SiRenombrando
        // (known/). Ahora el renombre solo ya cambia A: inconcluso (div/).
        let r01 = "function f() {\n  var arguments;\n  let result = \"FAIL\";\n  const o = { ...{ get prop() { result = \"PASS\"; } } };\n  console.log(arguments.length, result);\n  return o;\n}\nf(1, 2);\n";
        for preset in [Preset::CoreDefaults, Preset::CarrilA] {
            let (o, m, a, b) = ab(r01, preset);
            assert_ne!(b.stdout, a.stdout, "oxc ya no tiene el Issue A: buscar otro");
            let s = sloppy_only(&r, (Minifier::Oxc, preset), &o, &m, &a, &b);
            assert!(s.as_ref().is_err_and(|n| n.contains("ya cambia")), "{preset:?}: {s:?}");
        }
        // Miente (como c4 10-rt-sloppy-miente): el bug esta en la rama que el
        // estricto apaga. Con `arguments` renombrado sigue siendo inconcluso.
        let (o, m, a, b) = ab("function rec(v) { console.log(v); }\nfunction g() { var arguments = 1; return arguments; }\ng();\nfunction f() { return this; }\nif (f() === undefined) {\n  console.log(\"estricto\");\n} else {\n  (function (...[value = rec(\"rest\")]) {})();\n}\nconsole.log(\"fin\");\n", Preset::CoreDefaults);
        assert_eq!(a.stdout, "rest\nfin\n");
        assert_ne!(b.stdout, a.stdout);
        let s = sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("B != A'")), "{s:?}");
        // n-principio: n34 con un `var arguments` inofensivo (para tomar el camino del
        // renombre). El renombre solo no apaga el bug (sigue en la rama sloppy) y en estricto
        // B != A': inconcluso. Con `veredicto` (`mismo_camino`) era SiRenombrando, known/.
        let (o, m, a, b) = ab("function g() { var arguments = 1; return arguments; }\ng();\nfunction f() { return this; }\nvar r = f() === undefined ? \"s\" : (Math.cbrt(-8) < 0 ? \"s\" : \"x\");\nconsole.log(r);\n", Preset::CoreDefaults);
        assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("s\n", "x\n"));
        let s = sloppy_only(&r, (Minifier::Oxc, Preset::CoreDefaults), &o, &m, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("B != A'")), "{s:?}");
    }

    /// R2-H12: "no es estricto-valido" solo ante un SyntaxError; un RangeError
    /// de compilacion (anidamiento extremo) tiene su propia nota.
    #[test]
    fn rechazo_estricto_distingue_range_error() {
        use super::rechazo_estricto;
        use crate::runner;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = runner::Preload::write_tagged("r2h12").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let sintaxis = "\"use strict\";\nvar arguments = 1;\n";
        assert!(!r.node_accepts(sintaxis));
        assert_eq!(rechazo_estricto(&r, sintaxis), "no es estricto-valido: confirmar en el carril B");
        // R2-H03 (anidado-arrays): 20000 `[` que V8 no puede compilar.
        let hondo = format!("\"use strict\";\nvar x = {}0{};\n", "[".repeat(20000), "]".repeat(20000));
        assert!(!r.node_accepts(&hondo));
        let n = rechazo_estricto(&r, &hondo);
        assert!(n.contains("RangeError") && !n.contains("no es estricto-valido"), "{n}");
    }

    /// b1-b (R2-H02): M16 solo es concluyente si la corrida honda termina y B
    /// difiere de A solo en lineas que dependen de la pila.
    #[test]
    fn profundidad_concluyente() {
        use super::depth_recheck;
        use crate::{recheck::{Camino, Recheck}, runner::{Exec, Status}};
        // b7-1: B no cambia de camino con mas pila (lo de antes); `distinto`, abajo.
        let igual = |_: &Exec| Camino::Igual;
        let ex = |status: Status, s: &str| Exec {
            status,
            stdout: s.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let ok = |s: &str| ex(Status::Exited(0), s);
        // t-principio: B con mas pila (`deep_b`) solo se corre si A depende de la pila.
        let nunca = || -> Exec { panic!("no hacia falta correr B con mas pila") };
        // n-ruido: A tal cual otra vez solo se corre si la corrida honda difiere de A y termino.
        let nunca_a = || -> Exec { panic!("no hacia falta correr A otra vez") };
        // A no depende de la pila: no hay flag.
        assert_eq!(depth_recheck(&ok("1\nx\n"), &ok("2\nx\n"), &ok("1\nx\n"), nunca_a, nunca, igual), None);
        // Solo cambia la linea que depende de la pila y con mas pila A y B dan lo mismo. Paso 7
        // (revision-tint B1): antes concluyente por `localizada`; ahora inconcluso (div/),
        // porque con la misma forma pasaba r09 (el bug en el catch del RangeError, que con mas
        // pila no corre). Gana el principio; no hay un positivo asi en el golden, la aceptacion
        // ni los known/ del corpus de SWC en oxc (MEDIDO).
        let r = depth_recheck(&ok("1200\nx\n"), &ok("1500\nx\n"), &ok("2400\nx\n"), || ok("1200\nx\n"), || ok("2400\nx\n"), igual);
        assert!(matches!(r, Some(Recheck::Inconcluso(_))), "{r:?}");
        let r = depth_recheck(&ok("-2\n"), &ok("NaN\n"), &ok("none\n"), || ok("-2\n"), || ok("none\n"), igual);
        assert!(matches!(r, Some(Recheck::Inconcluso(_))), "{r:?}");
        // t-principio: una cuenta de marcos sin tope (A y B con mas pila tampoco dan lo
        // mismo) ya no es concluyente.
        assert_eq!(depth_recheck(&ok("1200\nx\n"), &ok("1500\nx\n"), &ok("2400\nx\n"), || ok("1200\nx\n"), || ok("3000\nx\n"), igual), Some(Recheck::NoExplica));
        // B da lo mismo que A con mas pila (A se quedo sin pila, B no).
        assert_eq!(depth_recheck(&ok("corto\n"), &ok("largo\n"), &ok("largo\n"), || ok("corto\n"), || ok("largo\n"), igual), Some(Recheck::Explica));
        // b7-1 (revision n2, r04): lo mismo, pero B cambia de camino con mas pila (corre el catch
        // del RangeError con el bug y deja el valor por defecto): la coincidencia no explica.
        let distinto = |_: &Exec| Camino::Distinto("cobertura".into());
        let r = depth_recheck(&ok("corto\n"), &ok("largo\n"), &ok("largo\n"), || ok("corto\n"), || ok("largo\n"), distinto);
        assert!(matches!(r, Some(Recheck::Inconcluso(ref n)) if n.contains("todavia pasa por la violacion")), "{r:?}");
        // 07-profundidad-miente: ademas de la linea de la pila, B pierde otra (y con mas pila tambien).
        let r = depth_recheck(&ok("playo 3\n[ '__proto__' ]\n"), &ok("playo 5\n[]\n"), &ok("hondo 1\n[ '__proto__' ]\n"), || ok("playo 3\n[ '__proto__' ]\n"), || ok("hondo 1\n[]\n"), igual);
        assert_eq!(r, Some(Recheck::NoExplica));
        // t-principio (a07-profundidad-linea): el bug en la misma linea que la cuenta de pila.
        // Antes `localizada` lo aceptaba (Explica, known/).
        let r = depth_recheck(&ok("9000 PASS\n"), &ok("9500 FAIL\n"), &ok("18000 PASS\n"), || ok("9000 PASS\n"), || ok("18000 FAIL\n"), igual);
        assert_eq!(r, Some(Recheck::NoExplica));
        // R2-H02: la corrida honda en Timeout no es evidencia de profundidad.
        let r = depth_recheck(&ok("1\n"), &ok("2\n"), &ex(Status::Timeout, ""), nunca_a, nunca, igual);
        assert!(matches!(r, Some(Recheck::Inconcluso(ref n)) if n.contains("Timeout")), "{r:?}");
        // Un Failed del fuzzer sigue sin opinar.
        assert_eq!(depth_recheck(&ok("1\n"), &ok("2\n"), &ex(Status::Failed("x".into()), ""), nunca_a, nunca, igual), None);
        // n-ruido (t-real, Timeout con `expiry` de reloj real): A con mas pila da otra salida,
        // pero A tal cual tambien. Antes: B con mas pila coincidia por azar con A con mas pila
        // y la pila "explicaba" (known/). Ahora inconcluso, sin correr B con mas pila.
        let r = depth_recheck(&ok("expiry 45\n"), &ok("expiry 46\n"), &ok("expiry 47\n"), || ok("expiry 44\n"), nunca, igual);
        assert!(matches!(r, Some(Recheck::Inconcluso(ref n)) if n.contains("no es determinista")), "{r:?}");
        // Si A tal cual falla por el fuzzer, sigue el camino de siempre.
        let r = depth_recheck(&ok("corto\n"), &ok("largo\n"), &ok("largo\n"), || ex(Status::Failed("x".into()), ""), || ok("largo\n"), igual);
        assert_eq!(r, Some(Recheck::Explica));
    }

    /// b4-a (R1-H04 / C3), de punta a punta con oxc: una TDZ atrapada que oxc borra
    /// va a known/ (re-chequeo de violaciones Explica); la misma al lado de un bug
    /// real (O4, #24331) queda en div/ con `tdz_atrapada_irrelevante`.
    #[test]
    fn tdz_atrapada_de_punta_a_punta() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-viol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let (preset, kind) = (minify::Preset::CoreDefaults, minify::SourceKind::Script);
        let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let preload = runner::Preload::write_tagged("viol-pap").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = preset.build(Minifier::Oxc, kind);
        let tdz = "try { x; console.log(\"sin tdz\"); } catch (e) { console.log(\"tdz\"); }\nlet x = 1;\n";
        // I2 (revision-b456): la TDZ NO atrapada tambien pasa por el re-chequeo. oxc borra
        // `x;` (asume que no hay TDZ): known/. r09-tdz-final-getter: el Issue A de
        // ENVIAR.md (el spread con getter se mueve) y `x; let x;` al final: antes known/
        // ("No TDZ Violation") sin re-chequeo, ahora div/ (`tdz_irrelevante`).
        let nc = "console.log(\"a\");\nx;\nlet x = 1;\n";
        let r09 = "let result = \"FAIL\";\nconst o = { ...{ get prop() { result = \"PASS\"; } } };\nconsole.log(result);\nconsole.log(o);\nx;\nlet x;\n";
        for (rel, src, known_div) in [
            ("pos", tdz.to_string(), (1, 0)),
            ("neg", format!("{tdz}console.log(delete (0 / 0));\n"), (0, 1)),
            ("pos-nc", nc.to_string(), (1, 0)),
            ("neg-nc", r09.to_string(), (0, 1)),
        ] {
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(&src, &cfg) else { panic!("no minifica") };
            let mut st = Stats::default();
            let p = format!("{rel}/input.js");
            assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, r.run(&original), false, None, false, &[], None));
            assert_eq!((st.known, st.divergences), known_div, "{rel}");
        }
        let meta = std::fs::read_to_string(d.join("known/pos/meta.txt")).unwrap();
        assert!(meta.contains("oxc: No TDZ Violation (atrapada)") && meta.contains("  violaciones: Explica (quedan tdz_atrapada)\n"), "{meta}");
        let meta = std::fs::read_to_string(d.join("div/neg/meta.txt")).unwrap();
        assert!(meta.contains("tdz_atrapada_irrelevante") && meta.contains("  violaciones: NoExplica"), "{meta}");
        let meta = std::fs::read_to_string(d.join("known/pos-nc/meta.txt")).unwrap();
        assert!(meta.contains("\nknown: sensor, asuncion: oxc: No TDZ Violation\n") && meta.contains("  violaciones: Explica (quedan tdz)\n"), "{meta}");
        let meta = std::fs::read_to_string(d.join("div/neg-nc/meta.txt")).unwrap();
        assert!(meta.contains("tdz_irrelevante") && meta.contains("  violaciones: NoExplica"), "{meta}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 9.2 (H2 y M1 de la revision del bloque 8), de punta a punta con oxc: un desborde de pila
    /// nativo usado como dato y un bug (`Number.isInteger(5e-324)` -> `true`, `"abc".substring(1,
    /// undefined)` -> `""`) que deja en B el valor de A con mas pila. Sin literales que salar
    /// (`[true, bug]`) o con la sal sin llegar a la linea (`[0, bug.length]`), con 9e52cef iban a
    /// known/; ahora div/ con nota. El positivo con la tabla en las ramas de un `?:` (la sal amplia)
    /// sigue en known/ y la traza dice que se confirmo con la sal.
    #[test]
    fn profundidad_salada_de_punta_a_punta() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-b92-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let (preset, kind) = (minify::Preset::CarrilA, minify::SourceKind::Script);
        let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let preload = runner::Preload::write_tagged("b92-pap").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = preset.build(Minifier::Oxc, kind);
        let neg = |tabla: &str| {
            format!("let a = [];\nfor (let i = 0; i < 7000; i++) a = [a];\nfunction h(v) {{\n  const t = {tabla};\n  console.log(t[+(typeof v !== \"string\")]);\n}}\nPromise.resolve(a).then(JSON.stringify).then(h, h);\n")
        };
        let pos = "let a = [];\nfor (let i = 0; i < 1500; i++) a = [a];\nfunction rec(n) {\n  var x1 = n + 1, x2 = x1 * 2, x3 = x2 - 3, x4 = x3 ^ 5, x5 = x4 | 7, x6 = x5 & 9, x7 = x6 + x1, x8 = x7 * x2;\n  if (n === 0) { try { JSON.stringify(a); return 0; } catch (e) { return 1; } }\n  return rec(n - 1);\n}\nconsole.log(rec(4800) ? \"desbordo\" : \"cupo\");\n";
        for (rel, src, known_div, en, linea) in [
            ("sin-sal", neg("[true, Number.isInteger(5e-324)]"), (0, 1), "div", "profundidad: Inconcluso -> profundidad_inconcluso: no hay literales que salar"),
            ("no-llega", neg("[0, \"abc\".substring(1, undefined).length]"), (0, 1), "div", "la sal no llega a la linea 1 de B"),
            ("positivo", pos.to_string(), (1, 0), "known", "profundidad: Explica (queda el flag profundidad; confirmado con los literales salados)\n"),
        ] {
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(&src, &cfg) else { panic!("no minifica") };
            let mut st = Stats::default();
            let p = format!("{rel}/input.js");
            assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, r.run(&original), false, None, false, &[], None));
            assert_eq!((st.known, st.divergences), known_div, "{rel}");
            let meta = std::fs::read_to_string(d.join(format!("{en}/{rel}/meta.txt"))).unwrap();
            assert!(meta.contains(linea), "{rel}: {meta}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 7.4 (Fase R I3): con mas de MAX_PAUSAS (256) excepciones atrapadas el sensor se
    /// apaga y A y B se comparan sin el (`sensor_tope`). Antes los positivos iban a div/
    /// (sin el sensor no hay `tdz_atrapada`); ahora A hereda lo que el sensor vio antes del
    /// tope y el re-chequeo de violaciones (sin el sensor, B == A') decide. Los negativos
    /// (bug real al lado, o en el catch de la TDZ) siguen en div/, y una TDZ despues del
    /// tope tambien (el sensor no la vio: limite documentado). Integracion 7.1 + 7.4: Explica
    /// exige ademas el camino de B (7.1): una corrida de B con el sensor (tope 4096) entera, que
    /// da lo mismo que B y tiene su misma cobertura de V8 sin el sensor, sin la violacion
    /// (`recheck::camino_sensor_i3`; `tope_y_camino_de_b` prueba los casos que no pasan).
    #[test]
    fn tdz_atrapada_pasado_el_tope() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-tope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let (preset, kind) = (minify::Preset::CoreDefaults, minify::SourceKind::Script);
        let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let preload = runner::Preload::write_tagged("viol-tope").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = preset.build(Minifier::Oxc, kind);
        let ruido = "for (let i = 0; i < 300; i++) { try { null.x; } catch (e) {} }\n";
        let tdz = "try { x; } catch (e) { n++; }\n";
        let getter = "const o = { ...{ get prop() { n = \"PASS\"; } } };\nconsole.log(n);\nconsole.log(o);\n";
        for (rel, src, known_div, paso) in [
            // Integracion 7.1 + 7.4: B tambien pasa el tope (el ruido sigue en el minificado); su
            // camino (7.1) sale de B con el sensor y tope 4096: entera, misma cobertura, sin la violacion.
            ("pos-antes", format!("let n = 0;\n{tdz}{ruido}console.log(n);\nlet x = 1;\n"), (1, 0), "  violaciones: Explica"),
            ("pos-loop", "let n = 0;\nfor (let i = 0; i < 300; i++) { try { x; } catch (e) { n++; } }\nconsole.log(n);\nlet x = 1;\n".into(), (1, 0), "  violaciones: Explica"),
            ("neg-getter", format!("let n = 0;\n{tdz}{ruido}let x = 1;\n{getter}"), (0, 1), "  violaciones: NoExplica"),
            ("neg-catch", "let r = 0;\nfor (let i = 0; i < 300; i++) { try { r = String(x); } catch (e) { r = Math.cbrt(-8); } }\nconsole.log(r);\nlet x = 1;\n".into(), (0, 1), "  violaciones: Inconcluso"),
            ("lim-despues", format!("let n = 0;\n{ruido}{tdz}console.log(n);\nlet x = 1;\n"), (0, 1), "  asunciones: ninguna"),
        ] {
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(&src, &cfg) else { panic!("no minifica") };
            let a = r.run(&original);
            assert_eq!(a.sensor_degradado(), Some(runner::SENSOR_TOPE), "{rel}: sin esto el test no prueba nada");
            let mut st = Stats::default();
            let p = format!("{rel}/input.js");
            assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, a, false, None, false, &[], None));
            assert_eq!((st.known, st.divergences, st.sensor_repetidos), (known_div.0, known_div.1, 1), "{rel}");
            let dir = if known_div.0 == 1 { "known" } else { "div" };
            let meta = std::fs::read_to_string(d.join(format!("{dir}/{rel}/meta.txt"))).unwrap();
            assert!(meta.contains(paso), "{rel}: {meta}");
            let heredo = meta.contains("-> A y B comparados sin el sensor (I3); violaciones que vio el sensor (7.4): A tdz ");
            assert_eq!(heredo, rel != "lim-despues", "{rel}: {meta}");
        }
        drop(r);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Integracion 7.1 + 7.4 (H3 de rev-7.1; rev-int H1), de punta a punta con oxc: casos de
    /// aceptacion de 7.4 (`23-t01`, `23-t02`), de 7.1 (`23-b71-r01`, con el ruido de 7.4 para que
    /// A y B pasen el tope) y de rev-int (`23-i01`, `23-i02`: el programa nota el sensor). A y B
    /// se comparan sin el sensor (I3) y el camino de B (7.1) sale de una corrida de B con el sensor
    /// (tope 4096) que tiene que correr entera, dar lo mismo que B y tener la misma cobertura de
    /// V8 que B sin el sensor (`recheck::camino_sensor_i3`): t01/t02 known/; r01 div/ (B todavia
    /// pasa por la TDZ con el bug); t02 con 5000 excepciones div/ (tampoco entera); i01/i02 div/
    /// (con el sensor B toma otro camino). Ningun bug real a known/; algun positivo se pierde.
    #[test]
    fn tope_y_camino_de_b() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-tope-camino-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let (preset, kind) = (minify::Preset::CoreDefaults, minify::SourceKind::Script);
        let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let preload = runner::Preload::write_tagged("tope-camino").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = preset.build(Minifier::Oxc, kind);
        let leer = |c: &str| std::fs::read_to_string(format!("{}/tests/aceptacion/rechequeos/corpus/{c}/input.js", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let ruido = |n: u32| format!("for (let i = 0; i < {n}; i++) {{ try {{ null.x; }} catch (e) {{}} }}\n");
        let r01 = leer("23-b71-r01-tdz-vacio");
        let r01_tope = r01.replacen("console.log", &format!("{}console.log", ruido(300)), 1);
        let t02_5000 = leer("23-t02-tope-tdz-antes-positivo").replace("i < 1000", "i < 5000");
        assert_ne!(r01, r01_tope);
        // (caso, programa, known/div, texto que tiene que estar en meta.txt)
        for (rel, src, known_div, paso) in [
            ("t01", leer("23-t01-tope-tdz-loop-positivo"), (1, 0), "  violaciones: Explica"),
            ("t02", leer("23-t02-tope-tdz-antes-positivo"), (1, 0), "  violaciones: Explica"),
            ("r01-tope", r01_tope, (0, 1), "el sensor ve en B la violacion tdz"),
            ("t02-5000", t02_5000, (0, 1), "tampoco corrio entero (sensor_tope)"),
            ("i01", leer("23-i01-sensor-detectado-tope-b"), (0, 1), "con el sensor B toma otro camino que sin el"),
            ("i02", leer("23-i02-sensor-detectado-b-entero"), (0, 1), "con el sensor B toma otro camino que sin el"),
            // Integracion 7.1b: el cambio de camino esta solo en el lado derecho de un `&&=` (la cobertura
            // de V8 no lo separa); lo separan los contadores de rama de 7.1b.
            ("i03", leer("23-i03-sensor-detectado-andasig"), (0, 1), "con el sensor B toma otro camino que sin el"),
        ] {
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(&src, &cfg) else { panic!("no minifica") };
            let a = r.run(&original);
            assert_eq!(a.sensor_degradado(), Some(runner::SENSOR_TOPE), "{rel}: sin esto el test no prueba nada");
            let mut st = Stats::default();
            let p = format!("{rel}/input.js");
            assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, a, false, None, false, &[], None), "{rel}");
            assert_eq!((st.known, st.divergences, st.sensor_repetidos), (known_div.0, known_div.1, 1), "{rel}");
            let dir = if known_div.0 == 1 { "known" } else { "div" };
            let meta = std::fs::read_to_string(d.join(format!("{dir}/{rel}/meta.txt"))).unwrap();
            assert!(meta.contains(paso), "{rel}: {meta}");
        }
        drop(r);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// t-principio: "un bug real nunca termina en known/; solo un re-chequeo
    /// concluyente manda ahi". `coercion`, `builtins`, `fn_tostring`/`stack` (paso
    /// contenido) y `profundidad` (bug en la misma linea) iban a known/ con el Issue A
    /// de ENVIAR.md al lado (MEDIDO con el binario de la fase R). Por flag: el
    /// positivo legitimo sigue en known/ y el adversarial va a div/ con el paso en la traza.
    #[test]
    fn flags_sin_rechequeo_de_punta_a_punta() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-principio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let (preset, kind) = (minify::Preset::CoreDefaults, minify::SourceKind::Script);
        let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
        let preload = runner::Preload::write_tagged("principio").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = preset.build(Minifier::Oxc, kind);
        // Issue A de ENVIAR.md: el spread con getter se mueve (oxc imprime "FAIL").
        let bug = "let result = \"FAIL\";\nconst o = { ...{ get prop() { result = \"PASS\"; } } };\nconsole.log(result);\nconsole.log(o);\n";
        let casos: [(&str, String, bool, &str); 22] = [
            // n-principio (noche2/n-principio/corpus-ataque): el bug en la rama que solo corre con
            // la violacion (A=-2, B=NaN). Con `veredicto` iban a known/ (`localizada`).
            ("extends-adentro", "let r = \"none\";\ntry { class A extends 1 {} r = typeof A; } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\n".into(), false, "  extends: Inconcluso"),
            ("with-adentro", "var o = { k: 1 };\nvar r;\nwith (o) { r = typeof k !== \"undefined\" ? Math.cbrt(-8) : \"nok\"; }\nconsole.log(r);\n".into(), false, "  with: Inconcluso"),
            ("global-adentro", "var r;\ntry { r = window.location ? \"nav\" : \"x\"; } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\n".into(), false, "  global_conocido: Inconcluso"),
            ("violacion-adentro", "let r = \"none\";\ntry { r = String(x); } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\nlet x = 1;\n".into(), false, "  violaciones: Inconcluso"),
            ("sloppy-adentro", "function f() { return this; }\nvar r = f() === undefined ? \"estricto\" : Math.cbrt(-8);\nconsole.log(r);\n".into(), false, "  solo_sloppy: Inconcluso"),
            // ... y sus positivos (oxc usa la asuncion y B se comporta como A'): siguen en known/.
            ("extends-pos", "try { class A extends 1 {} console.log(\"no lanzo\"); } catch (e) { console.log(\"lanzo\"); }\n".into(), true, "  extends: Explica"),
            ("global-pos", "try { window; console.log(1); } catch (e) { console.log(2); }\n".into(), true, "  global_conocido: Explica"),
            // Paso 7 (revision-tint B1, corpus-adv r05/r10/r07/r09): el bug vive adentro de lo
            // que el re-chequeo neutraliza. Con el paso 6 iban a known/ (`localizada`).
            ("builtins-adentro", "Array.prototype.includes = function (v) { return Math.cbrt(-8); };\nvar arr = [1];\narr.push(2);\nconsole.log(arr.includes(2));\n".into(), false, "  builtins: Inconcluso"),
            ("coercion-adentro", "var o = { valueOf() { return Math.cbrt(-8); } };\nconsole.log(o + 1);\n".into(), false, "  coercion: Inconcluso"),
            ("tostring-adentro", "function f() { return 1; }\nlet result = \"none\";\nvar s = String(f);\nif (s.endsWith(\"}\") && !s.includes(\"native\")) { result = Math.cbrt(-8); }\nconsole.log(result);\n".into(), false, "  contenido: Inconcluso"),
            ("profundidad-adentro", "let n = 0;\nfunction r(k) { n++; if (k > 0) r(k - 1); }\nlet result = \"none\";\ntry { r(15000); } catch (e) { result = Math.cbrt(-8); }\nconsole.log(result);\n".into(), false, "  profundidad: Inconcluso"),
            // (caso, fuente, va a known/, linea de la traza)
            ("coercion-pos", "var o1 = { valueOf: function () { f1(); } };\nvar o2 = { valueOf: function () { f2(); } };\nvar r1 = o1 <= o2;\n".into(), true, "  coercion: Explica"),
            ("coercion-neg", format!("class P {{ toString() {{ return \"p\"; }} }}\n{bug}"), false, "  coercion: NoExplica -> coercion_irrelevante"),
            ("builtins-pos", "String.prototype.toUpperCase = function () { return \"pisado\"; };\nconsole.log(\"abc\".toUpperCase());\n".into(), true, "  builtins: Explica"),
            ("builtins-neg", format!("Array.prototype.ultimo = function () {{ return 1; }};\n{bug}"), false, "  builtins: NoExplica -> builtins_irrelevante"),
            ("tostring-pos", "function suma(primero, segundo) { return primero + segundo; }\nconsole.log(String(suma));\n".into(), true, "  contenido: Explica"),
            ("tostring-neg", format!("function f() {{ return 1; }}\nconsole.log(String(f).length);\n{bug}"), false, "  contenido: NoExplica -> contenido_irrelevante (con el contenido fijo"),
            // n-principio (n-ruido r03): un marco con otro nombre ya no es "contenido"; el mangling
            // de core_defaults (`at falla` -> `at e`) pasa a div/. El positivo es solo de posicion.
            // n-int: con n-ruido un `e.stack` impreso que solo cambia de posicion ya no diverge
            // (`compare` normaliza las posiciones); el positivo es un valor que sale de la columna
            // del marco, que `compare` no puede normalizar.
            ("stack-mangling", "function falla() {\n  var nada = null;\n  return nada.x;\n}\ntry { falla(); } catch (e) { console.log(e.stack); }\n".into(), false, "  contenido: Inconcluso"),
            // (oxc saca el bloque y el `new`: la columna pasa de 18 a 7; MEDIDO con el binario de n-int)
            ("stack-pos", "{ let errorObject = new Error(\"e\"); var s = errorObject.stack; }\nvar l = s.split(\"\\n\");\nconsole.log(l[1].endsWith(\":1:18)\"), l.length);\n".into(), true, "  contenido: Explica"),
            ("stack-nombre", "var ok = delete (0 / 0);\nfunction pasa() { return new Error(\"x\").stack; }\nfunction falla() { return new Error(\"x\").stack; }\nconsole.log((ok ? pasa : falla)());\n".into(), false, "  contenido: Inconcluso"),
            ("stack-neg", format!("try {{ null.x; }} catch (e) {{ console.log(e.stack.split(\"\\n\").length); }}\n{bug}"), false, "  contenido: NoExplica"),
            ("profundidad-neg", "let n = 0;\nfunction r() { n++; r(); }\ntry { r(); } catch (e) {}\nlet result = \"FAIL\";\nconst o = { ...{ get prop() { result = \"PASS\"; } } };\nconsole.log(n, result);\nconsole.log(o);\n".into(), false, "  profundidad: NoExplica"),
        ];
        for (rel, src, a_known, paso) in &casos {
            // Con los flags estaticos (`coercion` es uno), como en produccion.
            let minify::Outcome::Ok { original, minified, static_flags, .. } = minify::prepare(src, &cfg) else { panic!("no minifica: {rel}") };
            let mut st = Stats::default();
            let p = format!("{rel}/input.js");
            assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, r.run(&original), false, None, false, &static_flags, None), "{rel}: no diverge");
            assert_eq!((st.known, st.divergences), if *a_known { (1, 0) } else { (0, 1) }, "{rel}");
            let meta = std::fs::read_to_string(d.join(format!("{}/{rel}/meta.txt", if *a_known { "known" } else { "div" }))).unwrap();
            assert!(meta.contains(paso), "{rel}: {meta}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 8.3 (= 7.11, d2/difuzz-1, hono sse.js), de punta a punta con oxc: el programa imprime
    /// el TypeError del motor con el nombre del parametro (`cb is not a function`) y oxc lo
    /// manglea. Desde cd54ad2 iba a div/; ahora known/ por `mensaje_nombre`. Si ademas se
    /// imprime el stack, en core_defaults cambia el nombre del marco (`at run` -> `at e`) y
    /// sigue en div/ (n-principio, `stack-mangling`). Con el bug F (`"abc".substring(1, undefined)`
    /// -> ``) en otra linea, o dentro de la expresion que V8 escribe, sigue en div/ (la
    /// localizacion no pasa y el paso no corre).
    #[test]
    fn mensaje_nombre_de_punta_a_punta() {
        use super::Medido;
        use crate::{backend::Minifier, cache, compare_and_report, known, minify, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let run = |cuerpo: &str, atrapa: &str| format!("function run(cb) {{\n  try {{\n    {cuerpo}\n  }} catch (e) {{\n    {atrapa}\n  }}\n}}\nrun();\n");
        let f = "\"abc\".substring(1, undefined)";
        for preset in [minify::Preset::CarrilA, minify::Preset::CoreDefaults] {
            let d = std::env::temp_dir().join(format!("difuzz-test-mnombre-{}-{}", preset.name(), std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
            let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
            let cache = cache::CacheA::new(None, &[]);
            let triage = triage::Triage::load(&d);
            let kind = minify::SourceKind::Script;
            let medido = Some(Medido { minifier: Minifier::Oxc, preset, kind });
            let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido };
            let preload = runner::Preload::write_tagged(&format!("mnombre-{}", preset.name())).unwrap();
            let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
            let cfg = preset.build(Minifier::Oxc, kind);
            let casos: [(&str, String, bool); 5] = [
                ("mensaje", run("cb();", "console.log(e.message);"), true),
                ("error-con-stack", run("cb();", "console.error(e);"), preset == minify::Preset::CarrilA),
                ("constructor", run("new cb();", "console.log(String(e));"), true),
                ("bug-al-lado", format!("{}console.log({f});\n", run("cb();", "console.log(e.message);")), false),
                ("callee-con-bug", run(&format!("({f})(cb);"), "console.log(e.message);"), false),
            ];
            for (rel, src, a_known) in &casos {
                let minify::Outcome::Ok { original, minified, static_flags, .. } = minify::prepare(src, &cfg) else { panic!("no minifica: {rel}") };
                let mut st = Stats::default();
                let p = format!("{rel}/input.js");
                assert!(compare_and_report(&ctx, &r, &mut st, Path::new(&p), &original, &minified, r.run(&original), false, None, false, &static_flags, None), "{rel}: no diverge");
                assert_eq!((st.known, st.divergences), if *a_known { (1, 0) } else { (0, 1) }, "{} {rel}", preset.name());
                let meta = std::fs::read_to_string(d.join(format!("{}/{rel}/meta.txt", if *a_known { "known" } else { "div" }))).unwrap();
                if *a_known {
                    assert!(meta.contains("  mensaje_nombre: Explica") && meta.contains("difuzz: V8 escribe en el mensaje"), "{rel}: {meta}");
                } else {
                    assert!(!meta.contains("mensaje_nombre"), "{rel}: {meta}");
                }
            }
            std::fs::remove_dir_all(&d).unwrap();
        }
    }
}
