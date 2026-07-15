//! Estado y ciclo de vida del applet (equivalente al rol de Tray.ps1 +
//! la logica de ScreenSwitcher-WPF.ps1). El panel de COSMIC gestiona la
//! instancia y el autostart; aca solo queda el popup y los mensajes.

use std::time::{SystemTime, UNIX_EPOCH};

use cosmic::app::{Core, Task};
use cosmic::iced::core::window;
use cosmic::iced::platform_specific::shell::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{Limits, Subscription};
use cosmic::widget::Space;
use cosmic::Element;
use tokio::sync::watch::Sender;

use crate::brightness::{self, DdcInfo, EventToSub};
use crate::display::{self, OutputState};
use crate::profiles::{self, ProfilesConfig};

pub const APPID: &str = "dev.gaston.CosmicScreenSwitcher";

#[derive(Debug, Clone)]
pub struct SliderState {
    pub key: String,
    pub label: String,
    pub rank: usize,
    pub percent: u16,
}

#[derive(Debug, Clone)]
pub enum Status {
    Idle,
    Applying(String),
    Done(String),
    Error(String),
}

pub struct AppState {
    pub core: Core,
    popup: Option<Id>,
    last_close: Option<u128>,
    pub cfg: ProfilesConfig,
    pub active: Option<String>,
    pub applying: bool,
    pub status: Status,
    pub sliders: Vec<SliderState>,
    sender: Option<Sender<EventToSub>>,
}

#[derive(Debug, Clone)]
pub enum AppMsg {
    TogglePopup,
    ClosePopup,
    Apply(String),
    Applied(String, Result<(), String>),
    StateRead(Result<Vec<OutputState>, String>),
    SetBrightness(String, u16),
    DdcReady(Vec<DdcInfo>, Sender<EventToSub>),
    DdcRescanned(Vec<DdcInfo>),
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn refresh_task() -> Task<AppMsg> {
    cosmic::task::future(async {
        let res = tokio::task::spawn_blocking(display::read_state)
            .await
            .unwrap_or_else(|e| Err(format!("tarea interrumpida: {e}")));
        AppMsg::StateRead(res)
    })
}

impl AppState {
    fn send_ddc(&self, ev: EventToSub) {
        if let Some(s) = &self.sender {
            let _ = s.send(ev);
        }
    }

    /// Etiqueta y orden del slider segun el mapeo ddc-model de profiles.kdl
    /// (Principal arriba, Secundario abajo, como la v1.2 de Windows).
    fn slider_meta(&self, info: &DdcInfo) -> (String, usize) {
        for (i, m) in self.cfg.monitors.iter().enumerate() {
            if let Some(pat) = &m.ddc_model {
                if !pat.is_empty()
                    && info.model.to_lowercase().contains(&pat.to_lowercase())
                {
                    return (m.label.clone(), i);
                }
            }
        }
        let fallback = if info.model.is_empty() {
            info.key.clone()
        } else {
            info.model.clone()
        };
        (fallback, usize::MAX)
    }

    fn build_sliders(&self, infos: &[DdcInfo]) -> Vec<SliderState> {
        let mut sliders: Vec<SliderState> = infos
            .iter()
            .map(|info| {
                let (label, rank) = self.slider_meta(info);
                SliderState {
                    key: info.key.clone(),
                    label,
                    rank,
                    percent: info.percent,
                }
            })
            .collect();
        sliders.sort_by(|a, b| (a.rank, &a.label).cmp(&(b.rank, &b.label)));
        sliders
    }

    fn toggle_popup(&mut self) -> Task<AppMsg> {
        if let Some(id) = self.popup.take() {
            self.last_close = Some(now_ms());
            return destroy_popup(id);
        }
        // Si el compositor recien cerro el popup por el mismo click en el
        // icono, no reabrir (mismo guard que el applet de referencia).
        if self
            .last_close
            .map(|t| now_ms() - t < 200)
            .unwrap_or(false)
        {
            return Task::none();
        }

        // Recarga de perfiles editados a mano + refresco de estado/brillo,
        // como Show-SwitcherWindow en Windows.
        self.cfg = profiles::load();
        self.send_ddc(EventToSub::Refresh);

        let id = Id::unique();
        self.popup = Some(id);
        let mut settings = self.core.applet.get_popup_settings(
            self.core.main_window_id().unwrap(),
            id,
            None,
            None,
            None,
        );
        settings.positioner.size_limits = Limits::NONE
            .min_width(340.0)
            .max_width(420.0)
            .min_height(200.0)
            .max_height(760.0);
        Task::batch(vec![get_popup(settings), refresh_task()])
    }

    fn close_popup(&mut self) -> Task<AppMsg> {
        if let Some(id) = self.popup.take() {
            self.last_close = Some(now_ms());
            destroy_popup(id)
        } else {
            Task::none()
        }
    }
}

impl cosmic::Application for AppState {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ProfilesConfig;
    type Message = AppMsg;
    const APP_ID: &'static str = APPID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let state = AppState {
            core,
            popup: None,
            last_close: None,
            cfg: flags,
            active: None,
            applying: false,
            status: Status::Idle,
            sliders: Vec::new(),
            sender: None,
        };
        (state, refresh_task())
    }

    fn on_close_requested(&self, id: window::Id) -> Option<AppMsg> {
        if self.popup == Some(id) {
            return Some(AppMsg::ClosePopup);
        }
        None
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            AppMsg::TogglePopup => return self.toggle_popup(),
            AppMsg::ClosePopup => return self.close_popup(),
            AppMsg::Apply(name) => {
                if self.applying {
                    return Task::none();
                }
                if let Some(p) = self.cfg.profiles.iter().find(|p| p.name == name).cloned() {
                    self.applying = true;
                    self.status = Status::Applying(p.label.clone());
                    let label = p.label.clone();
                    return cosmic::task::future(async move {
                        let res = tokio::task::spawn_blocking(move || display::apply(&p))
                            .await
                            .unwrap_or_else(|e| Err(format!("tarea interrumpida: {e}")));
                        AppMsg::Applied(label, res)
                    });
                }
            }
            AppMsg::Applied(label, res) => {
                self.applying = false;
                self.status = match res {
                    Ok(()) => Status::Done(label),
                    Err(e) => Status::Error(e),
                };
                // Cambiar de perfil puede prender/apagar monitores DDC/CI.
                self.send_ddc(EventToSub::Refresh);
                return refresh_task();
            }
            AppMsg::StateRead(res) => match res {
                Ok(state) => {
                    self.active = display::detect_active(&self.cfg.profiles, &state);
                }
                Err(e) => {
                    if matches!(self.status, Status::Idle) {
                        self.status = Status::Error(e);
                    }
                }
            },
            AppMsg::SetBrightness(key, pct) => {
                if let Some(s) = self.sliders.iter_mut().find(|s| s.key == key) {
                    s.percent = pct;
                }
                self.send_ddc(EventToSub::Set(key, pct));
            }
            AppMsg::DdcReady(infos, sender) => {
                self.sliders = self.build_sliders(&infos);
                self.sender = Some(sender);
            }
            AppMsg::DdcRescanned(infos) => {
                self.sliders = self.build_sliders(&infos);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        self.applet_button_view()
    }

    fn view_window(&self, id: Id) -> Element<'_, Self::Message> {
        if self.popup != Some(id) {
            return Space::new().into();
        }
        self.core.applet.popup_container(self.popup_view()).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::run(brightness::sub)
    }
}
