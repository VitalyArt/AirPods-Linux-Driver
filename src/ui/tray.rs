use ksni::{Icon, ToolTip};
use tokio::sync::mpsc::UnboundedSender;

use crate::bluetooth::aacp::{BatteryStatus, ControlCommandIdentifiers};
use crate::ui::messages::BluetoothUIMessage;

#[derive(Debug, Default)]
pub struct MyTray {
    pub device_address: Option<bluer::Address>,
    pub text_mode: bool,
    pub conversation_detect_enabled: Option<bool>,
    pub battery_headphone: Option<u8>,
    pub battery_headphone_status: Option<BatteryStatus>,
    pub battery_l: Option<u8>,
    pub battery_l_status: Option<BatteryStatus>,
    pub battery_r: Option<u8>,
    pub battery_r_status: Option<BatteryStatus>,
    pub battery_c: Option<u8>,
    pub battery_c_status: Option<BatteryStatus>,
    pub connected: bool,
    pub listening_mode: Option<u8>,
    pub allow_off_option: Option<u8>,
    pub command_tx: Option<UnboundedSender<(ControlCommandIdentifiers, Vec<u8>)>>,
    pub ui_tx: Option<UnboundedSender<BluetoothUIMessage>>,
}

impl ksni::Tray for MyTray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }
    fn title(&self) -> String {
        "AirPods".into()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        [16, 22, 24, 32, 48, 64]
            .into_iter()
            .map(|size| generate_icon(self.visual_state(), self.text_mode, size))
            .collect()
    }
    fn tool_tip(&self) -> ToolTip {
        let mut lines = vec![if self.connected {
            "Connected".to_owned()
        } else {
            "Not connected".to_owned()
        }];
        if self.connected {
            for (label, level, status) in self.battery_components() {
                lines.push(component_description(label, level, status));
            }
            if let Some((label, _, _)) = self
                .listening_options()
                .iter()
                .find(|(_, value, _)| Some(*value) == self.listening_mode)
            {
                lines.push(format!("Listening Mode: {label}"));
            }
            if let Some(enabled) = self.conversation_detect_enabled {
                lines.push(format!(
                    "Conversation Awareness: {}",
                    if enabled { "On" } else { "Off" }
                ));
            }
        }
        ToolTip {
            title: "Airpods Driver · AirPods".into(),
            description: lines.join("\n"),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut menu = vec![
            StandardItem {
                label: "Open Airpods Driver".into(),
                icon_data: menu_icon(MenuIcon::App),
                activate: Box::new(|tray: &mut Self| {
                    if let Some(tx) = &tray.ui_tx {
                        let _ = tx.send(BluetoothUIMessage::OpenWindow);
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            info_item(
                if self.connected {
                    "AirPods · Connected"
                } else {
                    "AirPods · Not connected"
                },
                MenuIcon::App,
            ),
        ];
        if self.connected {
            for (label, level, status) in self.battery_components() {
                menu.push(info_item(
                    &component_description(label, level, status),
                    if status == Some(BatteryStatus::Charging) {
                        MenuIcon::Charging
                    } else {
                        MenuIcon::Battery
                    },
                ));
            }
            menu.push(MenuItem::Separator);
            menu.push(info_item("Listening Mode", MenuIcon::Noise));
            let options = self.listening_options();
            let selected = self
                .listening_mode
                .and_then(|mode| options.iter().position(|(_, value, _)| *value == mode))
                .unwrap_or(usize::MAX);
            let commands = options.clone();
            menu.push(
                RadioGroup {
                    // ksni checks each option's index; no match means no selected mode.
                    selected,
                    select: Box::new(move |tray: &mut Self, index| {
                        if let Some((_, value, _)) = commands.get(index) {
                            if *value != 1 || tray.allow_off_option == Some(1) {
                                tray.send_menu_command(
                                    ControlCommandIdentifiers::ListeningMode,
                                    *value,
                                );
                            }
                        }
                    }),
                    options: options
                        .into_iter()
                        .map(|(label, _, icon)| RadioItem {
                            label: label.into(),
                            icon_data: menu_icon(icon),
                            enabled: self.controls_ready(),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
            );
            menu.push(MenuItem::Separator);
            menu.push(
                CheckmarkItem {
                    label: "Conversation Awareness".into(),
                    icon_data: menu_icon(MenuIcon::Conversation),
                    checked: self.conversation_detect_enabled.unwrap_or(false),
                    enabled: self.controls_ready() && self.conversation_detect_enabled.is_some(),
                    activate: Box::new(|tray: &mut Self| {
                        if let Some(enabled) = tray.conversation_detect_enabled {
                            tray.send_menu_command(
                                ControlCommandIdentifiers::ConversationDetectConfig,
                                if enabled { 2 } else { 1 },
                            );
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            );
            if !self.controls_ready() {
                menu.push(info_item("Controls unavailable", MenuIcon::Off));
            }
        } else {
            menu.push(info_item("Connect in Bluetooth settings", MenuIcon::Off));
        }
        menu.extend([
            MenuItem::Separator,
            StandardItem {
                label: "Quit Airpods Driver".into(),
                icon_data: menu_icon(MenuIcon::Quit),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]);
        menu
    }
}

/// Clonable UI notification without exposing the service internals in debug logs.
#[derive(Clone)]
pub struct TrayHandle(pub ksni::Handle<MyTray>);
impl std::fmt::Debug for TrayHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrayHandle")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    Disconnected,
    Connected { level: Option<u8>, charging: bool },
}
fn valid_level(level: Option<u8>, status: Option<BatteryStatus>) -> Option<u8> {
    level.filter(|value| *value <= 100 && status != Some(BatteryStatus::Disconnected))
}
impl MyTray {
    pub fn accepts(&self, address: bluer::Address) -> bool {
        self.connected && self.device_address == Some(address)
    }
    pub fn connect(&mut self, address: bluer::Address) {
        self.clear_device();
        self.device_address = Some(address);
        self.connected = true;
    }
    pub fn disconnect(&mut self, address: bluer::Address) {
        if self.device_address == Some(address) {
            self.clear_device();
        }
    }
    fn clear_device(&mut self) {
        self.device_address = None;
        self.connected = false;
        self.battery_headphone = None;
        self.battery_headphone_status = None;
        self.battery_l = None;
        self.battery_l_status = None;
        self.battery_r = None;
        self.battery_r_status = None;
        self.battery_c = None;
        self.battery_c_status = None;
        self.listening_mode = None;
        self.allow_off_option = None;
        self.conversation_detect_enabled = None;
        self.command_tx = None;
    }
    pub fn visual_state(&self) -> VisualState {
        if !self.connected {
            return VisualState::Disconnected;
        }
        let components = [
            (self.battery_headphone, self.battery_headphone_status),
            (self.battery_l, self.battery_l_status),
            (self.battery_r, self.battery_r_status),
        ];
        let level = components
            .iter()
            .filter_map(|(level, status)| valid_level(*level, *status))
            .min();
        let charging = components
            .iter()
            .any(|(_, status)| *status == Some(BatteryStatus::Charging));
        VisualState::Connected { level, charging }
    }
}
fn component_description(label: &str, level: Option<u8>, status: Option<BatteryStatus>) -> String {
    let value = valid_level(level, status)
        .map(|v| format!("{v}%"))
        .unwrap_or_else(|| "—".into());
    format!(
        "{label}: {value}{}",
        if status == Some(BatteryStatus::Charging) {
            " · Charging"
        } else {
            ""
        }
    )
}

use image::{Rgba, RgbaImage};
const INK: Rgba<u8> = Rgba([40, 44, 52, 255]);
const PAPER: Rgba<u8> = Rgba([246, 247, 249, 255]);
const AMBER: Rgba<u8> = Rgba([230, 158, 43, 255]);

fn capsule(x: f32, y: f32, a: (f32, f32), b: (f32, f32), radius: f32) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy)).clamp(0., 1.);
    ((x - a.0 - t * vx).powi(2) + (y - a.1 - t * vy).powi(2)).sqrt() - radius
}
fn buds(x: f32, y: f32) -> f32 {
    let mut distance = f32::MAX;
    for mirror in [false, true] {
        let x = if mirror { 64. - x } else { x };
        let head = (((x - 18.).powi(2) / 81. + (y - 20.).powi(2) / 64.).sqrt() - 1.) * 8.;
        distance = distance.min(head.min(capsule(x, y, (22., 23.), (24., 47.), 3.8)));
    }
    distance
}
fn polygon_distance(x: f32, y: f32, points: &[(f32, f32)]) -> f32 {
    let mut inside = false;
    let mut distance = f32::MAX;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        distance = distance.min(capsule(x, y, a, b, 0.));
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside { -distance } else { distance }
}
fn paint_shape(img: &mut RgbaImage, shape: impl Fn(f32, f32) -> f32, fill: Rgba<u8>, outline: f32) {
    let scale = img.width() as f32 / 64.;
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let distance = shape((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        if distance <= outline {
            *pixel = if distance <= 0. {
                fill
            } else {
                Rgba([INK[0], INK[1], INK[2], fill[3]])
            };
        }
    }
}
fn render_icon(state: VisualState, text_mode: bool, size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size * 4, size * 4);
    let (level, charging) = match state {
        VisualState::Disconnected => (None, false),
        VisualState::Connected { level, charging } => (level, charging),
    };
    if text_mode {
        use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
        let font = FontRef::try_from_slice(include_bytes!("../../assets/font/DejaVuSans.ttf"))
            .expect("bundled font");
        let text = level.map(|v| v.to_string()).unwrap_or_else(|| "—".into());
        let scale = PxScale::from(
            img.width() as f32
                * if text.len() >= 3 && level.is_some() {
                    0.58
                } else {
                    0.74
                },
        );
        let scaled = font.as_scaled(scale);
        let width: f32 = text
            .chars()
            .map(|c| scaled.h_advance(font.glyph_id(c)))
            .sum();
        // Center actual ink vertically rather than the font's em box.
        let bounds: Vec<_> = text
            .chars()
            .filter_map(|c| font.outline_glyph(font.glyph_id(c).with_scale(scale)))
            .map(|g| g.px_bounds())
            .collect();
        let top = bounds.iter().map(|b| b.min.y).fold(f32::MAX, f32::min);
        let bottom = bounds.iter().map(|b| b.max.y).fold(f32::MIN, f32::max);
        let x = ((img.width() as f32 - width) / 2.).round() as i32;
        let y = ((img.height() as f32 - (bottom - top)) / 2.
            - top
            - scaled.ascent()
            - if charging {
                img.width() as f32 * 4. / 64.
            } else {
                0.
            })
        .round() as i32;
        let stroke = (img.width() / 64).max(1) as i32;
        for dx in -stroke..=stroke {
            for dy in -stroke..=stroke {
                imageproc::drawing::draw_text_mut(
                    &mut img,
                    INK,
                    x + dx,
                    y + dy,
                    scale,
                    &font,
                    &text,
                );
            }
        }
        imageproc::drawing::draw_text_mut(
            &mut img,
            if level.is_some_and(|v| v <= 20) && !charging {
                AMBER
            } else {
                PAPER
            },
            x,
            y,
            scale,
            &font,
            &text,
        );
    } else {
        let color = if state == VisualState::Disconnected {
            Rgba([PAPER[0], PAPER[1], PAPER[2], 145])
        } else {
            PAPER
        };
        paint_shape(&mut img, buds, color, 1.8);
        if state == VisualState::Disconnected {
            paint_shape(
                &mut img,
                |x, y| capsule(x, y, (13., 13.), (51., 52.), 1.7),
                PAPER,
                1.5,
            );
        }
        if level.is_none() && state != VisualState::Disconnected && !charging {
            paint_shape(
                &mut img,
                |x, y| ((x - 52.).powi(2) + (y - 51.).powi(2)).sqrt() - 5.,
                PAPER,
                1.8,
            );
            paint_shape(
                &mut img,
                |x, y| ((x - 52.).powi(2) + (y - 51.).powi(2)).sqrt() - 2.4,
                INK,
                0.,
            );
        } else if level.is_some_and(|v| v <= 20) && !charging {
            paint_shape(
                &mut img,
                |x, y| ((x - 52.).powi(2) + (y - 51.).powi(2)).sqrt() - 4.5,
                AMBER,
                1.8,
            );
        }
    }
    if charging {
        let bolt = if text_mode {
            [
                (34., 46.),
                (28., 54.),
                (32., 54.),
                (30., 60.),
                (38., 50.),
                (33., 50.),
            ]
        } else {
            [
                (53., 36.),
                (44., 49.),
                (50., 49.),
                (47., 59.),
                (59., 44.),
                (52., 44.),
            ]
        };
        paint_shape(&mut img, |x, y| polygon_distance(x, y, &bolt), PAPER, 1.8);
    }
    image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
}
fn generate_icon(state: VisualState, text_mode: bool, size: u32) -> Icon {
    let img = render_icon(state, text_mode, size);
    let data = img
        .pixels()
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect();
    Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;
    fn connected() -> MyTray {
        MyTray {
            connected: true,
            ..Default::default()
        }
    }
    #[test]
    fn percentage_ignores_invalid_disconnected_and_case_levels() {
        let tray = MyTray {
            battery_headphone: Some(255),
            battery_l: Some(80),
            battery_r: Some(12),
            battery_r_status: Some(BatteryStatus::Disconnected),
            battery_c: Some(1),
            ..connected()
        };
        assert_eq!(
            tray.visual_state(),
            VisualState::Connected {
                level: Some(80),
                charging: false
            }
        );
    }
    #[test]
    fn lowest_available_bud_and_real_zero_are_preserved() {
        let mut tray = MyTray {
            battery_l: Some(80),
            battery_r: Some(45),
            ..connected()
        };
        assert_eq!(
            tray.visual_state(),
            VisualState::Connected {
                level: Some(45),
                charging: false
            }
        );
        tray.battery_l = Some(0);
        assert_eq!(
            tray.visual_state(),
            VisualState::Connected {
                level: Some(0),
                charging: false
            }
        );
    }
    #[test]
    fn unknown_charge_is_not_empty_and_headphone_charging_is_supported() {
        let mut tray = connected();
        assert_eq!(
            tray.visual_state(),
            VisualState::Connected {
                level: None,
                charging: false
            }
        );
        tray.battery_headphone_status = Some(BatteryStatus::Charging);
        assert_eq!(
            tray.visual_state(),
            VisualState::Connected {
                level: None,
                charging: true
            }
        );
        assert!(
            tray.tool_tip()
                .description
                .contains("Headphones: — · Charging")
        );
    }
    #[test]
    fn disconnect_only_clears_the_tracked_device() {
        let a = "00:11:22:33:44:55".parse().unwrap();
        let b = "00:11:22:33:44:66".parse().unwrap();
        let mut tray = MyTray::default();
        tray.connect(a);
        tray.battery_l = Some(99);
        tray.disconnect(b);
        assert!(tray.accepts(a));
        assert_eq!(tray.battery_l, Some(99));
        tray.disconnect(a);
        assert!(!tray.accepts(a));
        assert_eq!(tray.visual_state(), VisualState::Disconnected);
        assert_eq!(tray.battery_l, None);
        assert!(!tray.tool_tip().description.contains("99"));
        tray.connect(b);
        assert!(!tray.accepts(a));
        assert!(tray.accepts(b));
    }
    #[test]
    fn low_charge_marker_includes_twenty_and_charging_takes_priority() {
        let state = |level, charging| VisualState::Connected {
            level: Some(level),
            charging,
        };
        assert_eq!(
            *render_icon(state(20, false), false, 64).get_pixel(52, 51),
            AMBER
        );
        assert_eq!(
            render_icon(state(21, false), false, 64).get_pixel(52, 51)[3],
            0
        );
        assert_eq!(
            render_icon(state(20, true), false, 24),
            render_icon(state(82, true), false, 24)
        );
    }
    #[test]
    fn pixmaps_have_all_sizes_transparent_corners_and_argb_channels() {
        let icons = connected().icon_pixmap();
        assert_eq!(
            icons.iter().map(|i| i.width).collect::<Vec<_>>(),
            vec![16, 22, 24, 32, 48, 64]
        );
        for icon in icons {
            assert_eq!(icon.height, icon.width);
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            assert_eq!(&icon.data[..4], &[0, 0, 0, 0]);
        }
        let icon = generate_icon(
            VisualState::Connected {
                level: Some(82),
                charging: false,
            },
            false,
            64,
        );
        let offset = (20 * 64 + 18) * 4;
        assert_eq!(
            &icon.data[offset..offset + 4],
            &[255, PAPER[0], PAPER[1], PAPER[2]]
        );
    }
    #[test]
    fn numerical_extremes_and_unknown_are_visible_without_clipping() {
        for level in [None, Some(0), Some(99), Some(100)] {
            let img = render_icon(
                VisualState::Connected {
                    level,
                    charging: false,
                },
                true,
                16,
            );
            assert!(img.pixels().any(|p| p[3] > 200));
            assert!((0..16).all(|n| img.get_pixel(n, 0)[3] == 0 && img.get_pixel(n, 15)[3] == 0));
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
enum MenuIcon {
    App,
    Battery,
    Charging,
    Noise,
    Transparency,
    Adaptive,
    Off,
    Conversation,
    Quit,
}
fn info_item(label: &str, icon: MenuIcon) -> ksni::MenuItem<MyTray> {
    ksni::menu::StandardItem {
        label: label.into(),
        enabled: false,
        icon_data: menu_icon(icon),
        ..Default::default()
    }
    .into()
}
impl MyTray {
    fn controls_ready(&self) -> bool {
        self.connected && self.command_tx.as_ref().is_some_and(|tx| !tx.is_closed())
    }
    fn send_menu_command(&self, identifier: ControlCommandIdentifiers, value: u8) {
        if self.controls_ready() {
            if let Some(tx) = &self.command_tx {
                let _ = tx.send((identifier, vec![value]));
            }
        }
    }
    fn listening_options(&self) -> Vec<(&'static str, u8, MenuIcon)> {
        let mut options = vec![
            ("Noise Cancellation", 2, MenuIcon::Noise),
            ("Transparency", 3, MenuIcon::Transparency),
            ("Adaptive", 4, MenuIcon::Adaptive),
        ];
        if self.allow_off_option == Some(1) {
            options.push(("Off", 1, MenuIcon::Off));
        }
        options
    }
    fn battery_components(&self) -> Vec<(&'static str, Option<u8>, Option<BatteryStatus>)> {
        let mut items = Vec::new();
        if self.battery_headphone.is_some() || self.battery_headphone_status.is_some() {
            items.push((
                "Headphones",
                self.battery_headphone,
                self.battery_headphone_status,
            ));
        }
        if items.is_empty()
            || self.battery_l.is_some()
            || self.battery_r.is_some()
            || self.battery_l_status.is_some()
            || self.battery_r_status.is_some()
        {
            items.extend([
                ("Left", self.battery_l, self.battery_l_status),
                ("Right", self.battery_r, self.battery_r_status),
            ]);
        }
        items.push(("Case", self.battery_c, self.battery_c_status));
        items
    }
}
fn menu_icon(kind: MenuIcon) -> Vec<u8> {
    use std::sync::OnceLock;
    static ICONS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    ICONS.get_or_init(|| {
        [
            MenuIcon::App,
            MenuIcon::Battery,
            MenuIcon::Charging,
            MenuIcon::Noise,
            MenuIcon::Transparency,
            MenuIcon::Adaptive,
            MenuIcon::Off,
            MenuIcon::Conversation,
            MenuIcon::Quit,
        ]
        .into_iter()
        .map(|kind| {
            let image = render_menu_icon(kind, 32);
            let mut bytes = std::io::Cursor::new(Vec::new());
            image
                .write_to(&mut bytes, image::ImageFormat::Png)
                .expect("encode menu icon");
            bytes.into_inner()
        })
        .collect()
    })[kind as usize]
        .clone()
}
fn render_menu_icon(kind: MenuIcon, size: u32) -> RgbaImage {
    let mut image = RgbaImage::new(size * 4, size * 4);
    let ring = |x: f32, y: f32, cx: f32, cy: f32, r: f32| {
        (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() - 1.8
    };
    paint_shape(
        &mut image,
        |x, y| {
            let line = |a, b| capsule(x, y, a, b, 1.8);
            match kind {
                MenuIcon::App => buds(x, y),
                MenuIcon::Battery => {
                    let rect = line((12., 20.), (48., 20.))
                        .min(line((12., 44.), (48., 44.)))
                        .min(line((12., 20.), (12., 44.)))
                        .min(line((48., 20.), (48., 44.)));
                    rect.min(capsule(x, y, (54., 28.), (54., 36.), 2.))
                        .min(capsule(x, y, (20., 28.), (20., 36.), 2.))
                        .min(capsule(x, y, (28., 28.), (28., 36.), 2.))
                }
                MenuIcon::Charging => polygon_distance(
                    x,
                    y,
                    &[
                        (36., 9.),
                        (17., 35.),
                        (30., 35.),
                        (26., 55.),
                        (48., 27.),
                        (34., 27.),
                    ],
                ),
                MenuIcon::Noise => line((15., 28.), (15., 36.))
                    .min(line((24., 18.), (24., 46.)))
                    .min(line((33., 12.), (33., 52.)))
                    .min(line((42., 18.), (42., 46.)))
                    .min(line((51., 28.), (51., 36.))),
                MenuIcon::Transparency => ring(x, y, 32., 32., 10.).min(ring(x, y, 32., 32., 22.)),
                MenuIcon::Adaptive => line((12., 42.), (23., 22.))
                    .min(line((23., 22.), (35., 42.)))
                    .min(line((35., 42.), (51., 17.))),
                MenuIcon::Off => ring(x, y, 32., 34., 20.)
                    .max(if y < 20. && (x - 32.).abs() < 9. {
                        1.
                    } else {
                        -100.
                    })
                    .min(line((32., 9.), (32., 32.))),
                MenuIcon::Conversation => line((13., 15.), (51., 15.))
                    .min(line((51., 15.), (51., 42.)))
                    .min(line((51., 42.), (29., 42.)))
                    .min(line((29., 42.), (16., 53.)))
                    .min(line((16., 53.), (16., 42.)))
                    .min(line((16., 42.), (13., 42.)))
                    .min(line((13., 42.), (13., 15.)))
                    .min(line((23., 25.), (41., 25.)))
                    .min(line((23., 33.), (35., 33.))),
                MenuIcon::Quit => line((13., 12.), (30., 12.))
                    .min(line((13., 12.), (13., 52.)))
                    .min(line((13., 52.), (30., 52.)))
                    .min(line((27., 32.), (53., 32.)))
                    .min(line((43., 22.), (53., 32.)))
                    .min(line((43., 42.), (53., 32.))),
            }
        },
        PAPER,
        1.6,
    );
    image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use ksni::{MenuItem, Tray};
    fn group(tray: &MyTray) -> ksni::menu::RadioGroup<MyTray> {
        tray.menu()
            .into_iter()
            .find_map(|i| match i {
                MenuItem::RadioGroup(group) => Some(group),
                _ => None,
            })
            .expect("listening modes")
    }
    #[test]
    fn unknown_mode_has_no_false_selection_and_known_mode_is_selected() {
        let mut tray = MyTray {
            connected: true,
            ..Default::default()
        };
        assert!(group(&tray).selected >= group(&tray).options.len());
        tray.listening_mode = Some(4);
        let menu = group(&tray);
        assert_eq!(menu.options[menu.selected].label, "Adaptive");
        assert!(menu.options.iter().all(|i| !i.enabled));
    }
    #[test]
    fn menu_commands_use_correct_values_and_wait_for_device_confirmation() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tray = MyTray {
            connected: true,
            command_tx: Some(tx),
            conversation_detect_enabled: Some(false),
            allow_off_option: Some(1),
            ..Default::default()
        };
        let modes = group(&tray);
        assert_eq!(modes.options.last().unwrap().label, "Off");
        (modes.select)(&mut tray, 3);
        assert_eq!(
            rx.try_recv().unwrap(),
            (ControlCommandIdentifiers::ListeningMode, vec![1])
        );
        (modes.select)(&mut tray, usize::MAX);
        assert!(rx.try_recv().is_err());
        let toggle = tray
            .menu()
            .into_iter()
            .find_map(|i| match i {
                MenuItem::Checkmark(i) => Some(i),
                _ => None,
            })
            .unwrap();
        assert_eq!(toggle.label, "Conversation Awareness");
        (toggle.activate)(&mut tray);
        assert_eq!(
            rx.try_recv().unwrap(),
            (ControlCommandIdentifiers::ConversationDetectConfig, vec![1])
        );
        assert_eq!(tray.conversation_detect_enabled, Some(false));
        tray.connected = false;
        (toggle.activate)(&mut tray);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn disconnected_menu_omits_controls_and_stale_battery() {
        let tray = MyTray {
            battery_l: Some(82),
            ..Default::default()
        };
        let items = tray.menu();
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, MenuItem::RadioGroup(_) | MenuItem::Checkmark(_)))
        );
        let labels: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Standard(i) => Some(i.label.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Connect in Bluetooth settings"));
        assert!(!labels.iter().any(|label| label.contains("82%")));
    }
    #[test]
    fn embedded_menu_icons_are_valid_transparent_pngs() {
        for kind in [
            MenuIcon::App,
            MenuIcon::Battery,
            MenuIcon::Charging,
            MenuIcon::Noise,
            MenuIcon::Transparency,
            MenuIcon::Adaptive,
            MenuIcon::Off,
            MenuIcon::Conversation,
            MenuIcon::Quit,
        ] {
            let image = image::load_from_memory(&menu_icon(kind))
                .unwrap()
                .into_rgba8();
            assert_eq!(image.dimensions(), (32, 32));
            assert_eq!(image.get_pixel(0, 0)[3], 0);
            assert!(image.pixels().any(|p| p[3] > 200));
        }
    }
}
