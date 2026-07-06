//! Motor de perfiles (equivalente a Apply-DisplayProfile.ps1).
//!
//! Habla con el compositor via la CLI nativa `cosmic-randr`, en fases como el
//! motor CCD de Windows:
//!   1. encender lo que falte (antes de apagar nada: nunca 0 pantallas),
//!   2. geometria por salida (modo + posicion + rotacion, de izq. a der.),
//!   3. apagar lo que sobre,
//!   4. primario (compat Xwayland),
//!   5. releer y corregir posiciones si el compositor corrio algo para evitar
//!      solapamientos transitorios (el apply no es atomico como Use-DisplayConfig).

use crate::profiles::{first_arg_str, node_args, prop_str, Profile};
use kdl::{KdlDocument, KdlNode};

#[derive(Debug, Clone, Default)]
pub struct OutputState {
    pub name: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub transform: String,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub primary: bool,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
}

// Sincronico a proposito: se llama desde spawn_blocking (UI) o desde --check.
fn randr(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("cosmic-randr")
        .args(args)
        .output()
        .map_err(|e| format!("no pude ejecutar cosmic-randr: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "cosmic-randr {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn child<'a>(node: &'a KdlNode, name: &str) -> Option<&'a KdlNode> {
    node.children()
        .and_then(|c| c.nodes().iter().find(|n| n.name().value() == name))
}

fn args_i64(node: &KdlNode) -> Vec<i64> {
    node_args(node)
        .iter()
        .filter_map(|v| v.as_integer())
        .map(|v| v as i64)
        .collect()
}

fn first_arg_bool(node: &KdlNode) -> Option<bool> {
    node_args(node).first().and_then(|v| v.as_bool())
}

pub fn parse_state(text: &str) -> Result<Vec<OutputState>, String> {
    let doc: KdlDocument = text
        .parse()
        .map_err(|e| format!("salida KDL de cosmic-randr invalida: {e}"))?;
    let mut res = Vec::new();

    for node in doc.nodes().iter().filter(|n| n.name().value() == "output") {
        let Some(name) = first_arg_str(node) else { continue };
        let mut st = OutputState {
            name,
            enabled: crate::profiles::prop_bool(node, "enabled").unwrap_or(false),
            transform: "normal".into(),
            ..Default::default()
        };
        if let Some(d) = child(node, "description") {
            st.make = prop_str(d, "make").unwrap_or_default();
            st.model = prop_str(d, "model").unwrap_or_default();
        }
        if let Some(p) = child(node, "position") {
            let a = args_i64(p);
            if a.len() >= 2 {
                st.x = a[0] as i32;
                st.y = a[1] as i32;
            }
        }
        if let Some(t) = child(node, "transform") {
            if let Some(s) = first_arg_str(t) {
                st.transform = s;
            }
        }
        if let Some(x) = child(node, "xwayland_primary") {
            st.primary = first_arg_bool(x).unwrap_or(false);
        }
        if let Some(s) = child(node, "serial_number") {
            st.serial = first_arg_str(s);
        }
        if let Some(modes) = child(node, "modes").and_then(|m| m.children()) {
            for m in modes.nodes().iter().filter(|n| n.name().value() == "mode") {
                let current = crate::profiles::prop_bool(m, "current").unwrap_or(false);
                if current {
                    let a = args_i64(m);
                    if a.len() >= 2 {
                        st.width = a[0] as u32;
                        st.height = a[1] as u32;
                    }
                    if a.len() >= 3 {
                        st.refresh_mhz = a[2] as u32;
                    }
                }
            }
        }
        res.push(st);
    }
    Ok(res)
}

pub fn read_state() -> Result<Vec<OutputState>, String> {
    let text = randr(&["list", "--kdl"])?;
    parse_state(&text)
}

/// Compara el estado real contra cada perfil (como Get-ActiveProfileName):
/// mismo set de salidas encendidas + misma rotacion + mismas posiciones
/// relativas. Las posiciones se normalizan restando el minimo, porque el
/// origen absoluto del layout puede variar (p.ej. queda corrido cuando se
/// apaga la salida de mas a la izquierda). No se compara el primario
/// (xwayland_primary puede no estar seteado antes del primer apply).
pub fn detect_active(profiles: &[Profile], state: &[OutputState]) -> Option<String> {
    let on: Vec<&OutputState> = state.iter().filter(|o| o.enabled).collect();
    if on.is_empty() {
        return None;
    }
    let min_x = on.iter().map(|o| o.x).min().unwrap_or(0);
    let min_y = on.iter().map(|o| o.y).min().unwrap_or(0);

    'profiles: for p in profiles {
        let specs: Vec<_> = p.outputs.iter().filter(|s| s.enabled).collect();
        if specs.len() != on.len() {
            continue;
        }
        let smin_x = specs.iter().map(|s| s.x).min().unwrap_or(0);
        let smin_y = specs.iter().map(|s| s.y).min().unwrap_or(0);
        for spec in &specs {
            let Some(o) = on.iter().find(|o| o.name == spec.connector) else {
                continue 'profiles;
            };
            if o.transform != spec.transform
                || o.x - min_x != spec.x - smin_x
                || o.y - min_y != spec.y - smin_y
            {
                continue 'profiles;
            }
        }
        return Some(p.name.clone());
    }
    None
}

pub fn apply(profile: &Profile) -> Result<(), String> {
    let state = read_state()?;
    let on_specs: Vec<_> = profile.outputs.iter().filter(|s| s.enabled).collect();
    if on_specs.is_empty() {
        return Err("el perfil no enciende ningun monitor".into());
    }

    // Toda salida requerida tiene que estar conectada ahora mismo.
    for spec in &on_specs {
        if !state.iter().any(|o| o.name == spec.connector) {
            let have: Vec<_> = state.iter().map(|o| o.name.as_str()).collect();
            return Err(format!(
                "no encuentro la salida '{}' (conectadas: {})",
                spec.connector,
                have.join(", ")
            ));
        }
    }

    // Fase 1: encender lo que falte.
    for spec in &on_specs {
        let cur = state.iter().find(|o| o.name == spec.connector).unwrap();
        if !cur.enabled {
            randr(&["enable", &spec.connector])?;
        }
    }

    // Fase 2: geometria por salida, de izquierda a derecha.
    let mut ordered = on_specs.clone();
    ordered.sort_by_key(|s| s.x);
    for spec in &ordered {
        let w = spec.width.to_string();
        let h = spec.height.to_string();
        let x = spec.x.to_string();
        let y = spec.y.to_string();
        let mut a: Vec<&str> = vec![
            "mode",
            &spec.connector,
            &w,
            &h,
            "--pos-x",
            &x,
            "--pos-y",
            &y,
            "--transform",
            &spec.transform,
        ];
        let refresh_s;
        if let Some(hz) = spec.refresh {
            refresh_s = format!("{hz}");
            a.push("--refresh");
            a.push(&refresh_s);
        }
        randr(&a)?;
    }

    // Fase 3: apagar lo que sobre.
    for o in state.iter().filter(|o| o.enabled) {
        if !on_specs.iter().any(|s| s.connector == o.name) {
            randr(&["disable", &o.name])?;
        }
    }

    // Fase 4: primario.
    if let Some(p) = on_specs.iter().find(|s| s.primary) {
        randr(&["xwayland", "--primary", &p.connector])?;
    }

    // Fase 5: releer y corregir posiciones que hayan quedado corridas.
    let after = read_state()?;
    for spec in &ordered {
        if let Some(o) = after.iter().find(|o| o.name == spec.connector) {
            if o.enabled && (o.x != spec.x || o.y != spec.y) {
                randr(&[
                    "position",
                    &spec.connector,
                    &spec.x.to_string(),
                    &spec.y.to_string(),
                ])?;
            }
        }
    }

    Ok(())
}
