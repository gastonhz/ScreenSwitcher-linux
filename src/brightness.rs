//! Brillo por DDC/CI (equivalente a Brightness.ps1).
//!
//! Corre como worker dentro de una Subscription de iced: detecta los
//! monitores DDC/CI con reintentos que ACUMULAN resultados (las lecturas son
//! flaky, igual que en Windows: en una pasada puede responder uno y en la
//! siguiente el otro), normaliza a 0-100 segun el maximo real del monitor, y
//! aplica cambios en background. Cada `Refresh` (apertura del popup, apply de
//! perfil) re-enumera de cero ademas de releer valores, asi un monitor que
//! fallo al arranque se recupera solo. El canal `watch` conserva solo el
//! ultimo valor pedido, que hace de debounce natural al arrastrar el slider.
//! Los monitores que nunca responden (p.ej. la TV Samsung) se omiten solos.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cosmic::iced::futures::{SinkExt, Stream};
use cosmic::iced::stream;
use ddc_hi::{Ddc, Display};
use tokio::sync::watch;

use crate::app::AppMsg;

const VCP_BRIGHTNESS: u8 = 0x10;

#[derive(Debug, Clone)]
pub struct DdcInfo {
    pub key: String,
    pub model: String,
    pub percent: u16,
}

#[derive(Debug, Clone)]
pub enum EventToSub {
    Refresh,
    Set(String, u16),
}

struct Mon {
    display: Arc<Mutex<Display>>,
    max: u16,
    model: String,
    percent: u16,
}

fn to_percent(value: u16, max: u16) -> u16 {
    ((value as f32 / max.max(1) as f32) * 100.0).round().clamp(0.0, 100.0) as u16
}

/// Un intento de deteccion sobre `mons`: enumera los displays presentes,
/// prueba DDC/CI en cada uno y refresca/agrega los que responden. Un monitor
/// ya conocido que falla la lectura NO se descarta (conserva handle y ultimo
/// valor); uno nuevo que falla queda pendiente y devuelve `true` para que el
/// caller reintente. Los desconectados se podan.
fn scan_once(mons: &mut HashMap<String, Mon>) -> bool {
    let mut present = HashSet::new();
    let mut some_failed = false;

    for mut display in Display::enumerate() {
        let key = display.info.id.clone();
        present.insert(key.clone());
        match display.handle.get_vcp_feature(VCP_BRIGHTNESS) {
            Ok(v) => {
                let max = if v.maximum() == 0 { 100 } else { v.maximum() };
                mons.insert(
                    key,
                    Mon {
                        model: display.info.model_name.clone().unwrap_or_default(),
                        percent: to_percent(v.value(), max),
                        display: Arc::new(Mutex::new(display)),
                        max,
                    },
                );
            }
            Err(_) => {
                if !mons.contains_key(&key) {
                    some_failed = true;
                }
            }
        }
    }
    mons.retain(|k, _| present.contains(k));
    some_failed
}

/// Deteccion con reintentos que acumulan: lo que respondio en un intento
/// queda, y solo se reprueba lo que falto, con backoff. Asi una pasada flaky
/// no pisa a la anterior. Los que nunca responden (la TV) agotan los intentos
/// y se omiten.
async fn scan(mut mons: HashMap<String, Mon>, max_attempts: u32) -> HashMap<String, Mon> {
    let mut wait = Duration::from_millis(150);
    for attempt in 1..=max_attempts {
        let res = tokio::task::spawn_blocking(move || {
            let mut mons = mons;
            let failed = scan_once(&mut mons);
            (mons, failed)
        })
        .await;
        let (m, failed) = res.unwrap_or_else(|_| (HashMap::new(), false));
        mons = m;
        if !failed || attempt == max_attempts {
            break;
        }
        tokio::time::sleep(wait).await;
        wait *= 2;
    }
    mons
}

fn infos_of(mons: &HashMap<String, Mon>) -> Vec<DdcInfo> {
    mons.iter()
        .map(|(key, m)| DdcInfo {
            key: key.clone(),
            model: m.model.clone(),
            percent: m.percent,
        })
        .collect()
}

pub fn sub() -> impl Stream<Item = AppMsg> {
    stream::channel(
        64,
        |mut out: cosmic::iced::futures::channel::mpsc::Sender<AppMsg>| async move {
            // Deteccion inicial (arranque de sesion: DDC/CI mas flaky que
            // nunca, por eso mas intentos).
            let mut mons = scan(HashMap::new(), 4).await;

            let (tx, mut rx) = watch::channel(EventToSub::Refresh);
            rx.mark_unchanged();
            let _ = out.send(AppMsg::DdcReady(infos_of(&mons), tx)).await;

            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let ev = rx.borrow_and_update().clone();
                match ev {
                    EventToSub::Refresh => {
                        // Re-deteccion completa: recupera monitores que
                        // fallaron antes y poda los que se apagaron.
                        mons = scan(mons, 2).await;
                        let _ = out.send(AppMsg::DdcRescanned(infos_of(&mons))).await;
                    }
                    EventToSub::Set(key, pct) => {
                        if let Some(mon) = mons.get_mut(&key) {
                            mon.percent = pct.min(100);
                            let d = Arc::clone(&mon.display);
                            let raw = ((pct.min(100) as f32 / 100.0) * mon.max as f32).round() as u16;
                            let _ = tokio::task::spawn_blocking(move || {
                                d.lock().unwrap().handle.set_vcp_feature(VCP_BRIGHTNESS, raw)
                            })
                            .await;
                            // DDC/CI es lento: pequenio respiro entre escrituras.
                            tokio::time::sleep(Duration::from_millis(60)).await;
                        }
                    }
                }
            }
        },
    )
}
