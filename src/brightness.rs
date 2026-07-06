//! Brillo por DDC/CI (equivalente a Brightness.ps1).
//!
//! Corre como worker dentro de una Subscription de iced: enumera los
//! monitores DDC/CI con reintentos (las lecturas son flaky, igual que en
//! Windows), normaliza a 0-100 segun el maximo real del monitor, y aplica
//! cambios en background. El canal `watch` conserva solo el ultimo valor
//! pedido, que hace de debounce natural al arrastrar el slider. Los monitores
//! que no responden (p.ej. la TV Samsung por HDMI) se omiten solos.

use std::collections::HashMap;
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
}

fn to_percent(value: u16, max: u16) -> u16 {
    ((value as f32 / max.max(1) as f32) * 100.0).round().clamp(0.0, 100.0) as u16
}

fn enumerate() -> (HashMap<String, Mon>, Vec<DdcInfo>, bool) {
    let mut mons = HashMap::new();
    let mut infos = Vec::new();
    let mut some_failed = false;

    for mut display in Display::enumerate() {
        match display.handle.get_vcp_feature(VCP_BRIGHTNESS) {
            Ok(v) => {
                let max = if v.maximum() == 0 { 100 } else { v.maximum() };
                let key = display.info.id.clone();
                let model = display.info.model_name.clone().unwrap_or_default();
                infos.push(DdcInfo {
                    key: key.clone(),
                    model,
                    percent: to_percent(v.value(), max),
                });
                mons.insert(
                    key,
                    Mon {
                        display: Arc::new(Mutex::new(display)),
                        max,
                    },
                );
            }
            // Sin DDC/CI o lectura fallida: se reintenta la enumeracion entera
            // un par de veces y despues se lo da por no soportado.
            Err(_) => some_failed = true,
        }
    }
    (mons, infos, some_failed)
}

pub fn sub() -> impl Stream<Item = AppMsg> {
    stream::channel(
        64,
        |mut out: cosmic::iced::futures::channel::mpsc::Sender<AppMsg>| async move {
            let mut attempts = 0u32;
            let mut wait = Duration::from_millis(150);

            let (mons, infos) = loop {
                let (mons, infos, some_failed) =
                    tokio::task::spawn_blocking(enumerate)
                        .await
                        .unwrap_or_else(|_| (HashMap::new(), Vec::new(), true));
                attempts += 1;
                // Con que respondan los que soportan DDC/CI alcanza; si nada
                // respondio, reintento con backoff (arranque de sesion).
                if some_failed && mons.is_empty() && attempts < 4 {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                    continue;
                }
                break (mons, infos);
            };

            let (tx, mut rx) = watch::channel(EventToSub::Refresh);
            rx.mark_unchanged();
            let _ = out.send(AppMsg::DdcReady(infos, tx)).await;

            loop {
                if rx.changed().await.is_err() {
                    return;
                }
                let ev = rx.borrow_and_update().clone();
                match ev {
                    EventToSub::Refresh => {
                        for (key, mon) in &mons {
                            let d = Arc::clone(&mon.display);
                            let max = mon.max;
                            let res = tokio::task::spawn_blocking(move || {
                                d.lock().unwrap().handle.get_vcp_feature(VCP_BRIGHTNESS)
                            })
                            .await;
                            if let Ok(Ok(v)) = res {
                                let _ = out
                                    .send(AppMsg::DdcUpdated(key.clone(), to_percent(v.value(), max)))
                                    .await;
                            }
                        }
                    }
                    EventToSub::Set(key, pct) => {
                        if let Some(mon) = mons.get(&key) {
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
