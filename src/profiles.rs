//! Carga de perfiles desde KDL (equivalente a profiles.psd1 + su parseo).
//!
//! Se lee ~/.config/screen-switcher/profiles.kdl; si no existe o tiene
//! errores, se usa la copia embebida en el binario.

use kdl::{KdlDocument, KdlNode, KdlValue};
use std::path::PathBuf;

pub const DEFAULT_PROFILES: &str = include_str!("../profiles.kdl");

#[derive(Debug, Clone)]
pub struct MonitorDef {
    pub key: String,
    pub connector: String,
    pub label: String,
    /// Substring del model_name que reporta DDC/CI, para etiquetar sliders.
    pub ddc_model: Option<String>,
    /// Pintado ambar en el diagrama (rol TV), como en la UI de Windows.
    pub is_tv: bool,
}

#[derive(Debug, Clone)]
pub struct OutputSpec {
    pub monitor_key: String,
    pub connector: String,
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Hz (cosmic-randr matchea el modo con tolerancia de +-0.5 Hz).
    pub refresh: Option<f64>,
    pub transform: String,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub label: String,
    pub outputs: Vec<OutputSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct ProfilesConfig {
    pub monitors: Vec<MonitorDef>,
    pub profiles: Vec<Profile>,
}

// -- Helpers KDL compartidos con display.rs ---------------------------------

pub(crate) fn prop<'a>(node: &'a KdlNode, key: &str) -> Option<&'a KdlValue> {
    node.entry(key).map(|e| e.value())
}

pub(crate) fn prop_str(node: &KdlNode, key: &str) -> Option<String> {
    prop(node, key).and_then(|v| v.as_string()).map(str::to_owned)
}

pub(crate) fn prop_i64(node: &KdlNode, key: &str) -> Option<i64> {
    prop(node, key).and_then(|v| v.as_integer()).map(|v| v as i64)
}

pub(crate) fn prop_f64(node: &KdlNode, key: &str) -> Option<f64> {
    prop(node, key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

pub(crate) fn prop_bool(node: &KdlNode, key: &str) -> Option<bool> {
    prop(node, key).and_then(|v| v.as_bool())
}

/// Argumentos posicionales (entries sin nombre) del nodo.
pub(crate) fn node_args(node: &KdlNode) -> Vec<&KdlValue> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .map(|e| e.value())
        .collect()
}

pub(crate) fn first_arg_str(node: &KdlNode) -> Option<String> {
    node_args(node)
        .first()
        .and_then(|v| v.as_string())
        .map(str::to_owned)
}

// -- Parseo ------------------------------------------------------------------

pub fn parse(text: &str) -> Result<ProfilesConfig, String> {
    let doc: KdlDocument = text.parse().map_err(|e| format!("KDL invalido: {e}"))?;
    let mut cfg = ProfilesConfig::default();

    for node in doc.nodes() {
        match node.name().value() {
            "monitor" => {
                let key = first_arg_str(node).ok_or("nodo monitor sin clave")?;
                let connector = prop_str(node, "connector")
                    .ok_or_else(|| format!("monitor '{key}' sin connector"))?;
                cfg.monitors.push(MonitorDef {
                    label: prop_str(node, "label").unwrap_or_else(|| key.clone()),
                    ddc_model: prop_str(node, "ddc-model"),
                    is_tv: prop_bool(node, "tv").unwrap_or(false),
                    connector,
                    key,
                });
            }
            "profile" => {
                let name = first_arg_str(node).ok_or("nodo profile sin nombre")?;
                let label = prop_str(node, "label").unwrap_or_else(|| name.clone());
                let mut outputs = Vec::new();
                if let Some(children) = node.children() {
                    for ch in children.nodes() {
                        let mkey = first_arg_str(ch)
                            .ok_or_else(|| format!("salida sin monitor en '{name}'"))?;
                        let def = cfg
                            .monitors
                            .iter()
                            .find(|m| m.key == mkey)
                            .ok_or_else(|| {
                                format!("perfil '{name}': monitor desconocido '{mkey}'")
                            })?;
                        match ch.name().value() {
                            "output" => outputs.push(OutputSpec {
                                monitor_key: mkey.clone(),
                                connector: def.connector.clone(),
                                enabled: true,
                                width: prop_i64(ch, "width").unwrap_or(1920) as u32,
                                height: prop_i64(ch, "height").unwrap_or(1080) as u32,
                                refresh: prop_f64(ch, "refresh"),
                                transform: prop_str(ch, "transform")
                                    .unwrap_or_else(|| "normal".into()),
                                x: prop_i64(ch, "x").unwrap_or(0) as i32,
                                y: prop_i64(ch, "y").unwrap_or(0) as i32,
                                primary: prop_bool(ch, "primary").unwrap_or(false),
                            }),
                            "off" => outputs.push(OutputSpec {
                                monitor_key: mkey.clone(),
                                connector: def.connector.clone(),
                                enabled: false,
                                width: 0,
                                height: 0,
                                refresh: None,
                                transform: "normal".into(),
                                x: 0,
                                y: 0,
                                primary: false,
                            }),
                            other => {
                                return Err(format!(
                                    "nodo desconocido '{other}' en perfil '{name}'"
                                ))
                            }
                        }
                    }
                }
                cfg.profiles.push(Profile { name, label, outputs });
            }
            _ => {}
        }
    }

    if cfg.profiles.is_empty() {
        return Err("el archivo no define ningun perfil".into());
    }
    Ok(cfg)
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("screen-switcher").join("profiles.kdl")
}

pub fn load() -> ProfilesConfig {
    let text =
        std::fs::read_to_string(config_path()).unwrap_or_else(|_| DEFAULT_PROFILES.to_string());
    parse(&text).unwrap_or_else(|e| {
        eprintln!("profiles.kdl con errores ({e}); usando los perfiles embebidos");
        parse(DEFAULT_PROFILES).expect("los perfiles embebidos son invalidos")
    })
}
