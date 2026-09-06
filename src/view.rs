//! UI del popup (equivalente a ScreenSwitcher-WPF.ps1): un tile por perfil
//! con mini-diagrama a escala, resaltado del perfil activo, sliders de brillo
//! y linea de estado. Paleta de roles identica a la version Windows.

use cosmic::iced::{Alignment, Border, Color, Length};
use cosmic::widget::{button, column, container, divider, row, slider, text, Space};
use cosmic::Element;

use crate::app::{AppMsg, AppState, Status};
use crate::profiles::{OutputSpec, Profile};

// Paleta de la version Windows (ScreenSwitcher-WPF.ps1).
const ACCENT: Color = Color { r: 0.231, g: 0.510, b: 0.965, a: 1.0 }; // #3B82F6
const ACCENT_FILL: Color = Color { r: 0.118, g: 0.227, b: 0.373, a: 1.0 }; // #1E3A5F
const AMBER: Color = Color { r: 0.961, g: 0.620, b: 0.043, a: 1.0 }; // #F59E0B
const AMBER_FILL: Color = Color { r: 0.361, g: 0.275, b: 0.067, a: 1.0 }; // #5C4611
const GRAY: Color = Color { r: 0.612, g: 0.639, b: 0.686, a: 1.0 }; // #9CA3AF
const GRAY_FILL: Color = Color { r: 0.294, g: 0.333, b: 0.388, a: 1.0 }; // #4B5563

const DIAG_W: f32 = 96.0;
const DIAG_H: f32 = 44.0;

/// Rectangulo coloreado de tamanio fijo (un monitor del diagrama).
fn block<'a>(w: f32, h: f32, fill: Color, stroke: Color) -> Element<'a, AppMsg> {
    container(Space::new())
        .width(Length::Fixed(w))
        .height(Length::Fixed(h))
        .class(cosmic::theme::Container::Custom(Box::new(move |_| {
            cosmic::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                border: Border {
                    color: stroke,
                    width: 1.0,
                    radius: 2.0.into(),
                },
                ..Default::default()
            }
        })))
        .into()
}

/// Footprint efectivo: rotate90/270 intercambian ancho y alto (misma regla
/// que New-Diagram en Windows).
fn footprint(spec: &OutputSpec) -> (f32, f32) {
    match spec.transform.as_str() {
        "rotate90" | "rotate270" | "flipped90" | "flipped270" => {
            (spec.height as f32, spec.width as f32)
        }
        _ => (spec.width as f32, spec.height as f32),
    }
}

impl AppState {
    pub fn applet_button_view(&self) -> Element<'_, AppMsg> {
        self.core
            .applet
            .icon_button("video-display-symbolic")
            .on_press(AppMsg::TogglePopup)
            .into()
    }

    /// Mini-diagrama a escala del perfil. Todos los perfiles usan Y=0, asi
    /// que alcanza con una fila ordenada por X con los footprints escalados.
    fn diagram<'a>(&self, p: &Profile) -> Element<'a, AppMsg> {
        let mut on: Vec<&OutputSpec> = p.outputs.iter().filter(|s| s.enabled).collect();
        on.sort_by_key(|s| s.x);

        let total_w: f32 = on.iter().map(|s| footprint(s).0).sum();
        let max_h: f32 = on
            .iter()
            .map(|s| footprint(s).1)
            .fold(0.0, f32::max)
            .max(1.0);
        let gap = 2.0 * (on.len().saturating_sub(1)) as f32;
        let scale = ((DIAG_W - gap) / total_w.max(1.0)).min(DIAG_H / max_h);

        let mut r = row::with_capacity(on.len())
            .spacing(2)
            .align_y(Alignment::Start);
        for spec in on {
            let (fw, fh) = footprint(spec);
            let is_tv = self
                .cfg
                .monitors
                .iter()
                .any(|m| m.key == spec.monitor_key && m.is_tv);
            let (fill, stroke) = if is_tv {
                (AMBER_FILL, AMBER)
            } else if spec.primary {
                (ACCENT_FILL, ACCENT)
            } else {
                (GRAY_FILL, GRAY)
            };
            r = r.push(block(
                (fw * scale).max(4.0),
                (fh * scale).max(4.0),
                fill,
                stroke,
            ));
        }

        container(r)
            .width(Length::Fixed(DIAG_W + 4.0))
            .height(Length::Fixed(DIAG_H + 4.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    }

    fn tile<'a>(&'a self, p: &'a Profile) -> Element<'a, AppMsg> {
        let is_active = self.active.as_deref() == Some(p.name.as_str());
        let content = row::with_capacity(2)
            .spacing(12)
            .align_y(Alignment::Center)
            .push(self.diagram(p))
            .push(text(&p.label).size(14));

        let mut b = button::custom(content)
            .width(Length::Fill)
            .padding([6, 10])
            .class(if is_active {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Standard
            });
        if !self.applying {
            b = b.on_press(AppMsg::Apply(p.name.clone()));
        }
        b.into()
    }

    fn brightness_view(&self) -> Element<'_, AppMsg> {
        let mut col = column::with_capacity(self.sliders.len() + 1).spacing(6);
        col = col.push(text("BRILLO").size(11));

        if self.sliders.is_empty() {
            return col
                .push(text("Sin control de brillo (ningun monitor activo responde DDC/CI).").size(12))
                .into();
        }

        for s in &self.sliders {
            let key = s.key.clone();
            col = col.push(
                row::with_capacity(3)
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text(&s.label).size(13).width(Length::Fixed(90.0)))
                    .push(
                        slider(0..=100u16, s.percent, move |v| {
                            AppMsg::SetBrightness(key.clone(), v)
                        })
                        .width(Length::Fill),
                    )
                    .push(text(format!("{}", s.percent)).size(13).width(Length::Fixed(34.0))),
            );
        }
        col.into()
    }

    fn status_view(&self) -> Element<'_, AppMsg> {
        let msg = match &self.status {
            Status::Idle => match &self.active {
                Some(name) => {
                    let label = self
                        .cfg
                        .profiles
                        .iter()
                        .find(|p| p.name == *name)
                        .map(|p| p.label.clone())
                        .unwrap_or_else(|| name.clone());
                    format!("Activo: {label}")
                }
                None => "Elegi un perfil.".to_string(),
            },
            Status::Applying(l) => format!("Aplicando '{l}'..."),
            Status::Done(l) => format!("OK - '{l}' aplicado."),
            Status::Warn(l, w) => format!("'{l}' aplicado, con ajustes: {w}"),
            Status::Error(e) => format!("Error: {e}"),
        };
        text(msg).size(12).into()
    }

    pub fn popup_view(&self) -> Element<'_, AppMsg> {
        let mut tiles = column::with_capacity(self.cfg.profiles.len()).spacing(6);
        for p in &self.cfg.profiles {
            tiles = tiles.push(self.tile(p));
        }

        column::with_capacity(6)
            .padding(12)
            .spacing(10)
            .push(text("Screen Switcher").size(16))
            .push(tiles)
            .push(divider::horizontal::default())
            .push(self.brightness_view())
            .push(divider::horizontal::default())
            .push(self.status_view())
            .into()
    }
}
