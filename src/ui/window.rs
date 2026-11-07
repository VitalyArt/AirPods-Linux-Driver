use crate::bluetooth::aacp::{AACPEvent, ControlCommandIdentifiers};
use crate::bluetooth::managers::DeviceManagers;
use crate::bluetooth::proximity::NearbyAirPods;
use crate::devices::enums::*;
use crate::ui::design::*;
use crate::ui::messages::BluetoothUIMessage;
use crate::utils::{MyTheme, get_app_settings_path};
use iced::widget::{Space, column, container, row, scrollable, text, text_input};
use iced::{
    Center, Element, Font, Length, Settings, Size, Subscription, Task, Theme, daemon, window,
};
use log::{debug, error};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc::UnboundedReceiver};

pub fn start_ui(
    ui_rx: UnboundedReceiver<BluetoothUIMessage>,
    start_minimized: bool,
    managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
) -> iced::Result {
    let rx = Arc::new(Mutex::new(ui_rx));
    daemon(
        move || App::new(rx.clone(), start_minimized, managers.clone()),
        App::update,
        App::view,
    )
    .subscription(App::subscription)
    .theme(App::theme)
    .title(App::title)
    .settings(Settings {
        id: Some("com.vitalyart.airpods-driver".into()),
        fonts: vec![
            include_bytes!("../../assets/font/DejaVuSans.ttf")
                .as_slice()
                .into(),
        ],
        default_font: Font::with_name("DejaVu Sans"),
        default_text_size: 15.into(),
        ..Settings::default()
    })
    .run()
}

pub struct App {
    window: Option<window::Id>,
    nearby: HashMap<String, NearbyAirPods>,
    connecting: HashSet<String>,
    discovery_error: Option<String>,
    pairing_error: Option<String>,
    forget_confirm: Option<String>,
    forgetting: HashSet<String>,
    selected_tab: Tab,
    selected_device: Option<String>,
    devices: HashMap<String, DeviceData>,
    selected_theme: MyTheme,
    ui_rx: Arc<Mutex<UnboundedReceiver<BluetoothUIMessage>>>,
    bluetooth_state: BluetoothState,
    device_states: HashMap<String, DeviceState>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    tray_handle: Option<super::tray::TrayHandle>,
    tray_text_mode: bool,
    stem_control: bool,
    rename: String,
    about_open: bool,
    pending: HashMap<String, u64>,
    next_command: u64,
    event_versions: HashMap<(String, u8), u64>,
    errors: HashMap<String, String>,
    app_error: Option<String>,
}
pub struct BluetoothState {
    connected_devices: Vec<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    Devices,
    Device,
    Settings,
    DeviceSettings,
}
#[derive(Debug, Clone)]
pub enum Command {
    Mode(AirPodsNoiseControlMode),
    Conversation(bool),
    Volume(bool),
    AllowOff(bool),
    Rename(String),
}
#[derive(Debug, Clone)]
pub enum Message {
    RequestForget(String),
    CancelForget,
    ConfirmForget(String),
    ForgetFinished(String, Result<(), String>),
    NavigateBack,
    ConnectNearby(String),
    ConnectFinished(String, Result<(), String>),
    FocusNext(bool),
    TrayUpdated,
    WindowOpened(window::Id),
    WindowClosed(window::Id),
    SelectTab(Tab),
    SelectDevice(DeviceChoice),
    ThemeSelected(MyTheme),
    CopyToClipboard(String),
    BluetoothMessage(BluetoothUIMessage),
    TrayTextModeChanged(bool),
    StemControlChanged(bool),
    RenameEdited(String),
    CancelRename,
    ToggleAbout,
    Run(String, Command),
    CommandFinished(String, u64, u64, Command, Result<(), String>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    id: String,
    label: String,
}
impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl App {
    fn new(
        rx: Arc<Mutex<UnboundedReceiver<BluetoothUIMessage>>>,
        minimized: bool,
        managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    ) -> (Self, Task<Message>) {
        let settings = std::fs::read_to_string(get_app_settings_path())
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or_default();
        let theme = settings
            .get("appearance")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(MyTheme::Light);
        let (id, open) = if minimized {
            (None, Task::none())
        } else {
            let (id, task) = window::open(window_settings());
            (Some(id), task.map(Message::WindowOpened))
        };
        let devices = read_devices();
        let selected_device = settings
            .get("selected_device")
            .and_then(|v| v.as_str())
            .filter(|id| devices.contains_key(*id))
            .map(str::to_owned)
            .or_else(|| devices.keys().min().cloned());
        (
            Self {
                devices,
                window: id,
                nearby: HashMap::new(),
                connecting: HashSet::new(),
                discovery_error: None,
                pairing_error: None,
                forget_confirm: None,
                forgetting: HashSet::new(),
                selected_tab: Tab::Device,
                selected_device,
                selected_theme: theme,
                ui_rx: rx.clone(),
                bluetooth_state: BluetoothState {
                    connected_devices: vec![],
                },
                device_states: HashMap::new(),
                device_managers: managers,
                tray_handle: None,
                tray_text_mode: settings
                    .get("tray_text_mode")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                stem_control: settings
                    .get("stem_control")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                rename: String::new(),
                about_open: false,
                pending: HashMap::new(),
                next_command: 0,
                event_versions: HashMap::new(),
                errors: HashMap::new(),
                app_error: None,
            },
            Task::batch([open, Task::perform(wait_for_message(rx), |m| m)]),
        )
    }
    fn apply_devices(&mut self, devices: HashMap<String, DeviceData>) -> Task<Message> {
        let removed: Vec<_> = self
            .devices
            .keys()
            .filter(|mac| !devices.contains_key(*mac))
            .cloned()
            .collect();
        for mac in &removed {
            self.device_states.remove(mac);
            self.pending.remove(mac);
            self.errors.remove(mac);
            self.forgetting.remove(mac);
            self.connecting.remove(mac);
            self.event_versions.retain(|(id, _), _| id != mac);
            self.bluetooth_state
                .connected_devices
                .retain(|id| id != mac);
            self.nearby
                .retain(|id, d| id != mac && d.connect_address.as_deref() != Some(mac));
            if self.forget_confirm.as_ref() == Some(mac) {
                self.forget_confirm = None;
            }
        }
        self.devices = devices;
        if self
            .selected_device
            .as_ref()
            .is_none_or(|id| !self.devices.contains_key(id))
        {
            self.selected_device = self
                .devices
                .iter()
                .min_by(|(a, x), (b, y)| {
                    x.name
                        .to_lowercase()
                        .cmp(&y.name.to_lowercase())
                        .then(a.cmp(b))
                })
                .map(|(id, _)| id.clone());
            if self.selected_tab != Tab::Devices {
                self.selected_tab = Tab::Device;
            }
            self.about_open = false;
            self.rename.clear();
        }
        self.pairing_error = None;
        Task::none()
    }
    fn title(&self, _: window::Id) -> String {
        "Airpods Driver".into()
    }
    fn save_settings(&mut self) {
        let path = get_app_settings_path();
        let result = (|| -> Result<(), std::io::Error> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, serde_json::json!({"appearance": self.selected_theme, "selected_device": self.selected_device, "tray_text_mode": self.tray_text_mode, "stem_control": self.stem_control}).to_string())
        })();
        self.app_error = result
            .err()
            .map(|e| format!("Could not save preferences: {e}"));
    }
    fn refresh_tray(&self) -> Task<Message> {
        let Some(handle) = self.tray_handle.clone() else {
            return Task::none();
        };
        let mode = self.tray_text_mode;
        Task::perform(
            async move {
                handle.0.update(move |tray| tray.text_mode = mode).await;
            },
            |_| Message::TrayUpdated,
        )
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::RequestForget(mac) => {
                if self.devices.contains_key(&mac) {
                    self.forget_confirm = Some(mac);
                }
                Task::none()
            }
            Message::CancelForget => {
                self.forget_confirm = None;
                Task::none()
            }
            Message::ConfirmForget(mac) => {
                if self.forget_confirm.as_ref() != Some(&mac)
                    || !self.devices.contains_key(&mac)
                    || self.forgetting.contains(&mac)
                {
                    return Task::none();
                }
                self.forgetting.insert(mac.clone());
                self.errors.remove(&mac);
                let managers = self.device_managers.clone();
                let tray = self.tray_handle.as_ref().map(|h| h.0.clone());
                Task::perform(
                    async move {
                        let result =
                            crate::bluetooth::pairing::forget(&mac, &managers, &tray).await;
                        Message::ForgetFinished(mac, result)
                    },
                    |m| m,
                )
            }
            Message::ForgetFinished(mac, result) => {
                self.forgetting.remove(&mac);
                self.forget_confirm = None;
                match result {
                    Ok(()) => {
                        if let Ok(devices) = crate::utils::device_store::read() {
                            let task = self.apply_devices(devices);
                            self.save_settings();
                            return task;
                        }
                    }
                    Err(e) if self.devices.contains_key(&mac) => {
                        self.errors
                            .insert(mac, format!("Could not forget device: {e}"));
                    }
                    Err(_) => {}
                }
                Task::none()
            }
            Message::NavigateBack => {
                if self
                    .forget_confirm
                    .as_ref()
                    .is_some_and(|mac| !self.forgetting.contains(mac))
                {
                    self.forget_confirm = None;
                    Task::none()
                } else {
                    self.selected_tab = Tab::Device;
                    Task::none()
                }
            }
            Message::ConnectNearby(id) => {
                if self.connecting.contains(&id) {
                    return Task::none();
                }
                let device = self.nearby.get(&id);
                let Some(address) = device.and_then(|d| d.connect_address.clone()) else {
                    return Task::none();
                };
                self.connecting.insert(id.clone());
                self.errors.remove(&id);
                Task::perform(
                    async move {
                        let target = address.clone();
                        let result = crate::utils::device_store::run(address, async move {
                            crate::bluetooth::le::connect_airpods(&target).await
                        })
                        .await
                        .unwrap_or_else(|| {
                            Err("Connection cancelled because pairing was removed.".into())
                        });
                        Message::ConnectFinished(id, result)
                    },
                    |m| m,
                )
            }
            Message::ConnectFinished(id, result) => {
                if !self.connecting.remove(&id) {
                    return Task::none();
                }
                match result {
                    Ok(()) => {
                        if let Some(device) = self.nearby.get_mut(&id) {
                            device.connected = true;
                            device.paired = true;
                        }
                    }
                    Err(error) => {
                        self.errors.insert(id, error);
                    }
                }
                Task::none()
            }
            Message::TrayUpdated => Task::none(),
            Message::FocusNext(reverse) => {
                if reverse {
                    iced::widget::operation::focus_previous()
                } else {
                    iced::widget::operation::focus_next()
                }
            }
            Message::WindowOpened(id) => {
                self.window = Some(id);
                Task::none()
            }
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                }
                Task::none()
            }
            Message::SelectDevice(device) => {
                self.selected_device = Some(device.id);
                self.selected_tab = Tab::Device;
                self.about_open = false;
                self.save_settings();
                Task::none()
            }
            Message::SelectTab(tab) => {
                if tab == Tab::DeviceSettings {
                    self.rename = self
                        .selected_device
                        .as_ref()
                        .and_then(|id| self.device_states.get(id))
                        .map(|DeviceState::AirPods(s)| s.device_name.clone())
                        .unwrap_or_default();
                }
                self.selected_tab = tab;
                Task::none()
            }
            Message::ThemeSelected(theme) => {
                self.selected_theme = theme;
                self.save_settings();
                Task::none()
            }
            Message::TrayTextModeChanged(value) => {
                self.tray_text_mode = value;
                self.save_settings();
                self.refresh_tray()
            }
            Message::StemControlChanged(value) => {
                self.stem_control = value;
                self.save_settings();
                Task::none()
            }
            Message::CopyToClipboard(value) => iced::clipboard::write(value),
            Message::RenameEdited(value) => {
                self.rename = value;
                Task::none()
            }
            Message::CancelRename => {
                if let Some(DeviceState::AirPods(s)) = self
                    .selected_device
                    .as_ref()
                    .and_then(|id| self.device_states.get(id))
                {
                    self.rename = s.device_name.clone();
                }
                Task::none()
            }
            Message::ToggleAbout => {
                self.about_open = !self.about_open;
                Task::none()
            }
            Message::Run(mac, command) => {
                if self.pending.contains_key(&mac)
                    || !self.bluetooth_state.connected_devices.contains(&mac)
                {
                    return Task::none();
                }
                if matches!(&command, Command::Rename(name) if name.trim().is_empty()) {
                    return Task::none();
                }
                self.next_command += 1;
                let token = self.next_command;
                self.pending.insert(mac.clone(), token);
                self.errors.remove(&mac);
                let revision = *self
                    .event_versions
                    .get(&(mac.clone(), command_key(&command)))
                    .unwrap_or(&0);
                let managers = self.device_managers.clone();
                Task::perform(
                    async move {
                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(8),
                            send_command(managers, &mac, &command),
                        )
                        .await
                        .unwrap_or_else(|_| Err("The device did not respond. Try again.".into()));
                        Message::CommandFinished(mac, token, revision, command, result)
                    },
                    |m| m,
                )
            }
            Message::CommandFinished(mac, token, revision, command, result) => {
                if !finish_pending(&mut self.pending, &mac, token) {
                    return Task::none();
                }
                match result {
                    Err(e) => {
                        self.errors
                            .insert(mac, format!("Could not apply change. {e}"));
                    }
                    Ok(()) => {
                        if self
                            .event_versions
                            .get(&(mac.clone(), command_key(&command)))
                            .copied()
                            .unwrap_or(0)
                            == revision
                        {
                            if let Some(state) = self.device_states.get_mut(&mac) {
                                apply_command(state, &command);
                            }
                        }
                        if let Command::Rename(name) = command {
                            match crate::utils::device_store::update(|devices| {
                                if let Some(device) = devices.get_mut(&mac) {
                                    device.name = name.clone();
                                }
                            }) {
                                Ok(devices) => self.devices = devices,
                                Err(e) => {
                                    self.app_error =
                                        Some(format!("Name changed, but could not save it: {e}"))
                                }
                            }
                            self.rename = name;
                        }
                    }
                }
                Task::none()
            }
            Message::BluetoothMessage(ui_message) => {
                match ui_message {
                    BluetoothUIMessage::DevicesChanged(devices) => {
                        let previous = self.selected_device.clone();
                        let close = self.apply_devices(devices);
                        if self.selected_device != previous {
                            self.save_settings();
                        }
                        Task::batch([
                            close,
                            Task::perform(wait_for_message(self.ui_rx.clone()), |m| m),
                        ])
                    }
                    BluetoothUIMessage::PairingError(error) => {
                        self.pairing_error = Some(error);
                        Task::perform(wait_for_message(self.ui_rx.clone()), |m| m)
                    }
                    BluetoothUIMessage::PairingReady => {
                        self.pairing_error = None;
                        Task::perform(wait_for_message(self.ui_rx.clone()), |m| m)
                    }

                    BluetoothUIMessage::DiscoveryReady => {
                        self.discovery_error = None;
                        Task::perform(wait_for_message(self.ui_rx.clone()), |m| m)
                    }
                    BluetoothUIMessage::NearbyAirPods(device) => {
                        self.discovery_error = None;
                        let wait = Task::perform(wait_for_message(self.ui_rx.clone()), |m| m);
                        self.receive_nearby(device);
                        wait
                    }
                    BluetoothUIMessage::NearbyLost(id) => {
                        self.nearby.remove(&id);
                        Task::perform(wait_for_message(self.ui_rx.clone()), |m| m)
                    }
                    BluetoothUIMessage::DiscoveryError(error) => {
                        self.discovery_error = Some(error);
                        self.nearby.clear();
                        Task::perform(wait_for_message(self.ui_rx.clone()), |m| m)
                    }

                    BluetoothUIMessage::TrayReady(handle) => {
                        self.tray_handle = Some(handle);
                        Task::batch([
                            self.refresh_tray(),
                            Task::perform(wait_for_message(self.ui_rx.clone()), |m| m),
                        ])
                    }
                    BluetoothUIMessage::OpenWindow => {
                        let ui_rx = Arc::clone(&self.ui_rx);
                        let wait_task = Task::perform(wait_for_message(ui_rx), |msg| msg);
                        debug!("Opening main window...");
                        if let Some(window_id) = self.window {
                            Task::batch(vec![window::gain_focus(window_id), wait_task])
                        } else {
                            let settings = window_settings();
                            let (new_window_task, open_task) = window::open(settings);
                            self.window = Some(new_window_task);
                            Task::batch(vec![open_task.map(Message::WindowOpened), wait_task])
                        }
                    }
                    BluetoothUIMessage::DeviceConnected(mac) => {
                        if !crate::utils::device_store::read().is_ok_and(|d| d.contains_key(&mac)) {
                            return Task::perform(wait_for_message(self.ui_rx.clone()), |m| m);
                        }

                        let ui_rx = Arc::clone(&self.ui_rx);
                        let wait_task = Task::perform(wait_for_message(ui_rx), |msg| msg);
                        debug!(
                            "Device connected: {}. Adding to connected devices list",
                            mac
                        );
                        let mut already_connected = false;
                        for device in &self.bluetooth_state.connected_devices {
                            if device == &mac {
                                already_connected = true;
                                break;
                            }
                        }
                        if !already_connected {
                            self.bluetooth_state.connected_devices.push(mac.clone());
                        }

                        self.devices = read_devices();
                        self.errors.remove(&mac);
                        if self.selected_device.is_none()
                            || !self
                                .bluetooth_state
                                .connected_devices
                                .contains(self.selected_device.as_ref().unwrap())
                        {
                            self.selected_device = Some(mac.clone());
                            self.save_settings();
                        }

                        let type_ = self.devices.get(&mac).map(|d| d.type_.clone());
                        match type_ {
                            Some(DeviceType::AirPods) => {
                                let device_managers = self.device_managers.blocking_read();
                                let Some(device_manager) = device_managers.get(&mac) else {
                                    return wait_task;
                                };
                                let Some(aacp_manager) = device_manager.get_aacp() else {
                                    return wait_task;
                                };
                                let aacp_manager_state = aacp_manager.state.clone();
                                let state = aacp_manager_state.blocking_lock();
                                debug!("AACP manager found for AirPods device {}", mac);
                                let device_name = self
                                    .devices
                                    .get(&mac)
                                    .map(|device| device.name.clone())
                                    .unwrap_or_else(|| "Your headphones".into());
                                self.device_states.insert(mac.clone(), DeviceState::AirPods(AirPodsState {
                                    device_name,
                                    battery: state.battery_info.clone(),
                                    noise_control_mode: state.control_command_status_list.iter().find_map(|status| {
                                        if status.identifier == ControlCommandIdentifiers::ListeningMode {
                                            status.value.first().map(AirPodsNoiseControlMode::from_byte)
                                        } else {
                                            None
                                        }
                                    }).unwrap_or(AirPodsNoiseControlMode::Transparency),
                                    conversation_awareness_enabled: state.control_command_status_list.iter().any(|status| {
                                        status.identifier == ControlCommandIdentifiers::ConversationDetectConfig &&
                                        matches!(status.value.as_slice(), [0x01])
                                    }),
                                    personalized_volume_enabled: state.control_command_status_list.iter().any(|status| {
                                        status.identifier == ControlCommandIdentifiers::AdaptiveVolumeConfig &&
                                        matches!(status.value.as_slice(), [0x01])
                                    }),
                                    allow_off_mode: state.control_command_status_list.iter().any(|status| {
                                        status.identifier == ControlCommandIdentifiers::AllowOffOption &&
                                        matches!(status.value.as_slice(), [0x01])
                                    }),
                                }));
                            }
                            _ => {}
                        }

                        Task::batch(vec![wait_task])
                    }
                    BluetoothUIMessage::DeviceDisconnected(mac) => {
                        let ui_rx = Arc::clone(&self.ui_rx);
                        let wait_task = Task::perform(wait_for_message(ui_rx), |msg| msg);
                        debug!("Device disconnected: {}", mac);

                        self.bluetooth_state
                            .connected_devices
                            .retain(|device| device != &mac);

                        self.device_states.remove(&mac);

                        self.pending.remove(&mac);
                        self.errors.insert(
                            mac.clone(),
                            "Device disconnected. Connect it in Bluetooth settings.".into(),
                        );

                        Task::batch(vec![wait_task])
                    }
                    BluetoothUIMessage::AACPUIEvent(mac, event) => {
                        let ui_rx = Arc::clone(&self.ui_rx);
                        let wait_task = Task::perform(wait_for_message(ui_rx), |msg| msg);
                        debug!("AACP UI Event for {}: {:?}", mac, event);
                        if !self.bluetooth_state.connected_devices.contains(&mac) {
                            return wait_task;
                        }
                        if let AACPEvent::ControlCommand(status) = &event {
                            *self
                                .event_versions
                                .entry((mac.clone(), status.identifier as u8))
                                .or_default() += 1;
                        }
                        match event {
                            AACPEvent::ControlCommand(status) => match status.identifier {
                                ControlCommandIdentifiers::ListeningMode => {
                                    let mode = status
                                        .value
                                        .first()
                                        .map(AirPodsNoiseControlMode::from_byte)
                                        .unwrap_or(AirPodsNoiseControlMode::Transparency);
                                    if let Some(DeviceState::AirPods(state)) =
                                        self.device_states.get_mut(&mac)
                                    {
                                        state.noise_control_mode = mode;
                                    }
                                }
                                ControlCommandIdentifiers::ConversationDetectConfig => {
                                    let is_enabled = match status.value.as_slice() {
                                        [0x01] => true,
                                        [0x02] => false,
                                        _ => {
                                            error!(
                                                "Unknown Conversation Detect Config value: {:?}",
                                                status.value
                                            );
                                            false
                                        }
                                    };
                                    if let Some(DeviceState::AirPods(state)) =
                                        self.device_states.get_mut(&mac)
                                    {
                                        state.conversation_awareness_enabled = is_enabled;
                                    }
                                }
                                ControlCommandIdentifiers::AdaptiveVolumeConfig => {
                                    let is_enabled = match status.value.as_slice() {
                                        [0x01] => true,
                                        [0x02] => false,
                                        _ => {
                                            error!(
                                                "Unknown Adaptive Volume Config value: {:?}",
                                                status.value
                                            );
                                            false
                                        }
                                    };
                                    if let Some(DeviceState::AirPods(state)) =
                                        self.device_states.get_mut(&mac)
                                    {
                                        state.personalized_volume_enabled = is_enabled;
                                    }
                                }
                                ControlCommandIdentifiers::AllowOffOption => {
                                    let is_enabled = match status.value.as_slice() {
                                        [0x01] => true,
                                        [0x02] => false,
                                        _ => {
                                            error!(
                                                "Unknown Allow Off Option value: {:?}",
                                                status.value
                                            );
                                            false
                                        }
                                    };
                                    if let Some(DeviceState::AirPods(state)) =
                                        self.device_states.get_mut(&mac)
                                    {
                                        state.allow_off_mode = is_enabled;
                                    }
                                }
                                _ => {
                                    debug!("Unhandled Control Command Status: {:?}", status);
                                }
                            },
                            AACPEvent::BatteryInfo(battery_info) => {
                                if let Some(DeviceState::AirPods(state)) =
                                    self.device_states.get_mut(&mac)
                                {
                                    state.battery = battery_info;
                                    debug!("Updated battery info for {}: {:?}", mac, state.battery);
                                }
                            }
                            _ => {}
                        }
                        Task::batch(vec![wait_task])
                    }
                }
            }
        }
    }
    fn receive_nearby(&mut self, mut device: NearbyAirPods) {
        let id = device.id.clone();
        // Classic inquiry and LE advertisements can describe the same target.
        // A Classic update carries no battery/lid data; preserve the LE snapshot.
        if device.lid_counter.is_none()
            && !device.pairing
            && let Some(previous) = self.nearby.get(&id)
        {
            device.battery = previous.battery.clone();
            device.lid_open = previous.lid_open;
            device.lid_counter = previous.lid_counter;
            device.pairing = previous.pairing;
        }
        self.nearby.insert(id, device);
    }
    fn visible_nearby(&self) -> Vec<&NearbyAirPods> {
        let mut nearby: Vec<_> = self
            .nearby
            .values()
            .filter(|d| {
                d.connect_address.is_some()
                    && !self.devices.contains_key(&d.id)
                    && !self
                        .devices
                        .contains_key(d.connect_address.as_ref().unwrap_or(&d.id))
                    && !d.connected
                    && !self
                        .bluetooth_state
                        .connected_devices
                        .contains(d.connect_address.as_ref().unwrap_or(&d.id))
            })
            .collect();
        nearby.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        nearby
    }
    fn nearby_view(&self) -> Element<'_, Message> {
        let mut body = column![text("Add headphones").size(20)].spacing(12);
        if let Some(error) = &self.discovery_error {
            body = body.push(card(muted(error)));
        }
        let nearby = self.visible_nearby();
        if nearby.is_empty() {
            body = body.push(muted(
                "Open the case and enable pairing mode. Nearby AirPods will appear here.",
            ));
        }
        for device in nearby {
            let busy = self.connecting.contains(&device.id);
            let label = match (busy, device.paired) {
                (true, false) => "Pairing…",
                (true, true) => "Connecting…",
                (false, false) => "Pair",
                (false, true) => "Connect",
            };
            let mut entry = column![
                text(&device.name).size(18),
                muted(&device.id),
                action(
                    label,
                    (!busy && device.connect_address.is_some())
                        .then(|| Message::ConnectNearby(device.id.clone())),
                    true
                )
            ]
            .spacing(8);
            if let Some(error) = self.errors.get(&device.id) {
                entry = entry.push(muted(error));
            }
            if !device.paired {
                entry = entry.push(muted(
                    "Put your AirPods in pairing mode until the indicator flashes white.",
                ));
            }
            if device.connect_address.is_none() {
                entry = entry.push(muted("Waiting for a Bluetooth pairing address…"));
            }
            body = body.push(card(entry));
        }
        body.into()
    }
    fn view(&self, _: window::Id) -> Element<'_, Message> {
        if let Some(mac) = self
            .forget_confirm
            .as_ref()
            .filter(|mac| self.devices.contains_key(*mac))
        {
            let name = &self.devices[mac].name;
            let busy = self.forgetting.contains(mac);
            return container(scrollable(column![
                text("Forget this device?").size(26),
                text(name).size(20),
                muted("Bluetooth pairing with this computer will be removed. Pair the headphones again to reconnect."),
                row![action("Cancel", (!busy).then_some(Message::CancelForget), false),
                    action(if busy { "Forgetting…" } else { "Forget device" }, (!busy).then(|| Message::ConfirmForget(mac.clone())), false)].spacing(8)
            ].spacing(20).padding(24)))
                .width(Length::Fill).height(Length::Fill).into();
        }
        let devices = &self.devices;
        let mut choices: Vec<_> = devices
            .iter()
            .map(|(id, d)| DeviceChoice {
                id: id.clone(),
                label: format!(
                    "{}{}",
                    d.name,
                    if self.bluetooth_state.connected_devices.contains(id) {
                        " · Connected"
                    } else {
                        ""
                    }
                ),
            })
            .collect();
        choices.sort_by(|a, b| {
            a.label
                .to_lowercase()
                .cmp(&b.label.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        let home = self.selected_tab == Tab::Device;
        let header = row![
            if home {
                link("Change headphones", Message::SelectTab(Tab::Devices))
            } else {
                link("‹ Back", Message::SelectTab(Tab::Device))
            },
            Space::new().width(Length::Fill),
            link("Settings", Message::SelectTab(Tab::Settings))
        ]
        .spacing(12)
        .align_y(Center);
        let mut body = column![header].spacing(16);
        if let Some(error) = &self.pairing_error {
            body = body.push(card(text(error).size(13)));
        }
        if let Some(error) = &self.app_error {
            body = body.push(card(
                text(error)
                    .size(13)
                    .color(iced::Color::from_rgb8(195, 65, 55)),
            ));
        }
        match self.selected_tab {
            Tab::Devices => {
                body = body.push(text("Headphones").size(28))
                    .push(muted("Choose headphones to view their battery and settings."))
                    .push(text("Saved headphones").size(20));
                if choices.is_empty() {
                    body = body.push(muted("No saved headphones yet."));
                }
                for choice in choices {
                    let current = self.selected_device.as_ref() == Some(&choice.id);
                    body = body.push(card(column![
                        text(choice.label.clone()).size(18),
                        muted(choice.id.clone()),
                        action(if current { "Selected · Open" } else { "Select headphones" },
                            Some(Message::SelectDevice(choice)), current)
                    ].spacing(8)));
                }
                body = body.push(self.nearby_view());
            }
            Tab::Settings => {
                body = body.push(text("Preferences").size(28)).push(muted("Make yourself at home."))
                    .push(text("Appearance").size(15))
                    .push(row![action("Light", Some(Message::ThemeSelected(MyTheme::Light)), self.selected_theme == MyTheme::Light), action("Dark", Some(Message::ThemeSelected(MyTheme::Dark)), self.selected_theme == MyTheme::Dark)].spacing(8))
                    .push(setting("Battery text in tray", "Show a percentage instead of the AirPods icon.", self.tray_text_mode, true, Message::TrayTextModeChanged))
                    .push(setting("Stem track controls", "Double press to skip; triple press to go back. Turn off if your desktop already handles these gestures.", self.stem_control, true, Message::StemControlChanged));
            }
            _ => {
                if let Some(id) = &self.selected_device {
                    let connected = self.bluetooth_state.connected_devices.contains(id);
                    let name = self
                        .device_states
                        .get(id)
                        .map(|DeviceState::AirPods(s)| s.device_name.as_str())
                        .or_else(|| devices.get(id).map(|d| d.name.as_str()))
                        .unwrap_or("Your headphones")
                        .to_owned();
                    let busy = self.pending.contains_key(id);
                    if let Some(error) = self.errors.get(id) {
                        body = body.push(card(
                            text(error)
                                .size(13)
                                .color(iced::Color::from_rgb8(195, 65, 55)),
                        ));
                    }
                    if self.selected_tab == Tab::DeviceSettings {
                        body = body.push(text("Device settings").size(26));
                        if let Some(state) = self.device_states.get(id).filter(|_| connected) {
                            let DeviceState::AirPods(state) = state;
                            let mut input = text_input("Device name", &self.rename)
                                .padding(12)
                                .style(input_style);
                            if !busy {
                                input = input.on_input(Message::RenameEdited);
                                if !self.rename.trim().is_empty() {
                                    input = input.on_submit(Message::Run(
                                        id.clone(),
                                        Command::Rename(self.rename.trim().into()),
                                    ));
                                }
                            }
                            body = body.push(card(column![text("Name").size(14), input, row![action("Cancel", (!busy).then_some(Message::CancelRename), false), action("Save", (!busy && !self.rename.trim().is_empty() && self.rename.trim() != state.device_name).then(|| Message::Run(id.clone(), Command::Rename(self.rename.trim().into()))), true)].spacing(8)].spacing(12)))
                                .push(setting("Personalized Volume", "Adjust volume to your surroundings.", state.personalized_volume_enabled, !busy, |v| Message::Run(id.clone(), Command::Volume(v))))
                                .push(setting("Off listening mode", "Allow turning noise control off. Loud sounds are not reduced in this mode.", state.allow_off_mode, !busy, |v| Message::Run(id.clone(), Command::AllowOff(v))));
                        } else {
                            body = body
                                .push(card(muted("Connect this device to change its settings.")));
                        }
                        body = body.push(card(column![
                            text("Forget this device").size(18),
                            muted("Remove pairing from this computer and Airpods Driver. Pair again to reconnect."),
                            action("Forget device…", (!self.forgetting.contains(id)).then(|| Message::RequestForget(id.clone())), false)
                        ].spacing(12)));
                        body = body.push(action(
                            if self.about_open {
                                "About this device  −"
                            } else {
                                "About this device  +"
                            },
                            Some(Message::ToggleAbout),
                            false,
                        ));
                        if self.about_open {
                            body = body.push(device_information(devices.get(id), id));
                        }
                    } else {
                        body = body
                            .push(
                                container(super::design::headphones())
                                    .width(Length::Fill)
                                    .center_x(Length::Fill),
                            )
                            .push(
                                column![
                                    text(name).size(24),
                                    muted(if connected {
                                        "Connected · Ready to listen"
                                    } else {
                                        "Not connected"
                                    })
                                ]
                                .spacing(6)
                                .align_x(Center)
                                .width(Length::Fill),
                            );
                        if !connected {
                            body = body.push(card(column![text("A little quiet here").size(18), muted("Connect your headphones in Bluetooth settings. Your controls will appear here.")].spacing(8)));
                        } else if let Some(state) = self.device_states.get(id) {
                            match state {
                                DeviceState::AirPods(state) => {
                                    body = body
                                        .push(super::airpods::battery_view(&state.battery))
                                        .push(super::airpods::controls(id, state, busy));
                                }
                            }
                            body = body.push(action(
                                "Device Settings  ›",
                                Some(Message::SelectTab(Tab::DeviceSettings)),
                                false,
                            ));
                        } else {
                            body = body.push(card(muted("Reading device information…")));
                        }
                    }
                    if self.selected_tab == Tab::Device
                        && (!connected || !self.device_states.contains_key(id))
                    {
                        body = body.push(action(
                            "Device Settings  ›",
                            Some(Message::SelectTab(Tab::DeviceSettings)),
                            false,
                        ));
                    }
                    if busy {
                        body = body.push(muted("Applying change…"));
                    }
                } else {
                    body = body.push(card(column![
                        text("Add your headphones").size(24),
                        muted("Pair AirPods to see their battery and listening controls."),
                        action("Add headphones", Some(Message::SelectTab(Tab::Devices)), true)
                    ].spacing(12)));
                }
            }
        }
        container(scrollable(body.padding(24).width(Length::Fill)).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    fn theme(&self, _: window::Id) -> Theme {
        self.selected_theme.into()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::close_events().map(Message::WindowClosed),
            iced::keyboard::listen().filter_map(|event| match event {
                iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
                    modifiers,
                    ..
                } => Some(Message::FocusNext(modifiers.shift())),
                iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                } => Some(Message::NavigateBack),
                _ => None,
            }),
        ])
    }
}

fn window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(460., 640.),
        min_size: Some(Size::new(380., 520.)),
        decorations: true,
        icon: app_icon(),
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    {
        settings.platform_specific.application_id = "com.vitalyart.airpods-driver".into();
    }
    settings
}
fn read_devices() -> HashMap<String, DeviceData> {
    crate::utils::device_store::read().unwrap_or_default()
}
async fn wait_for_message(rx: Arc<Mutex<UnboundedReceiver<BluetoothUIMessage>>>) -> Message {
    match rx.lock().await.recv().await {
        Some(m) => Message::BluetoothMessage(m),
        None => std::future::pending().await,
    }
}
async fn send_command(
    managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    mac: &str,
    command: &Command,
) -> Result<(), String> {
    let aacp = {
        let managers = managers.read().await;
        let manager = managers.get(mac).ok_or("Device is no longer connected.")?;
        manager.get_aacp()
    };
    let aacp = aacp.ok_or("Device controls are not ready.")?;
    let (identifier, value) = match command {
        Command::Rename(name) => {
            return aacp
                .send_rename_packet(name)
                .await
                .map_err(|e| e.to_string());
        }
        Command::Mode(mode) => (ControlCommandIdentifiers::ListeningMode, mode.to_byte()),
        Command::Conversation(v) => (
            ControlCommandIdentifiers::ConversationDetectConfig,
            if *v { 1 } else { 2 },
        ),
        Command::Volume(v) => (
            ControlCommandIdentifiers::AdaptiveVolumeConfig,
            if *v { 1 } else { 2 },
        ),
        Command::AllowOff(v) => (
            ControlCommandIdentifiers::AllowOffOption,
            if *v { 1 } else { 2 },
        ),
    };
    aacp.send_control_command(identifier, &[value])
        .await
        .map_err(|e| e.to_string())
}
fn finish_pending(pending: &mut HashMap<String, u64>, mac: &str, token: u64) -> bool {
    if pending.get(mac) != Some(&token) {
        return false;
    }
    pending.remove(mac);
    true
}
fn apply_command(state: &mut DeviceState, command: &Command) {
    match (state, command) {
        (DeviceState::AirPods(s), Command::Mode(v)) => s.noise_control_mode = v.clone(),
        (DeviceState::AirPods(s), Command::Conversation(v)) => {
            s.conversation_awareness_enabled = *v
        }
        (DeviceState::AirPods(s), Command::Volume(v)) => s.personalized_volume_enabled = *v,
        (DeviceState::AirPods(s), Command::AllowOff(v)) => s.allow_off_mode = *v,
        (DeviceState::AirPods(s), Command::Rename(v)) => s.device_name = v.clone(),
    }
}
fn device_information<'a>(device: Option<&DeviceData>, mac: &str) -> Element<'a, Message> {
    let mut fields = vec![("Bluetooth address", mac.to_string())];
    match device.and_then(|d| d.information.as_ref()) {
        Some(DeviceInformation::AirPods(info)) => fields.extend([
            ("Model", info.model_number.clone()),
            ("Manufacturer", info.manufacturer.clone()),
            ("Serial number", info.serial_number.clone()),
            ("Left serial", info.left_serial_number.clone()),
            ("Right serial", info.right_serial_number.clone()),
            ("Version 1", info.version1.clone()),
            ("Version 2", info.version2.clone()),
            ("Version 3", info.version3.clone()),
        ]),
        None => {}
    }
    let mut content = column![].spacing(12);
    for (label, value) in fields {
        content = content.push(
            column![
                muted(label),
                link(
                    if value.is_empty() {
                        "Unavailable".into()
                    } else {
                        value.clone()
                    },
                    Message::CopyToClipboard(value)
                )
            ]
            .spacing(4),
        );
    }
    card(content).into()
}
fn app_icon() -> Option<window::Icon> {
    let image = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .ok()?
        .into_rgba8();
    let (w, h) = image.dimensions();
    window::icon::from_rgba(image.into_raw(), w, h).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut app, _) = App::new(
            Arc::new(Mutex::new(rx)),
            true,
            Arc::new(RwLock::new(HashMap::new())),
        );
        app.devices.clear();
        app.selected_device = Some("pods".into());
        app.selected_tab = Tab::Device;
        app.bluetooth_state.connected_devices = vec!["pods".into()];
        app.device_states.insert("pods".into(), state());
        app
    }
    fn saved(name: &str) -> DeviceData {
        DeviceData {
            name: name.into(),
            type_: DeviceType::AirPods,
            information: None,
        }
    }
    #[test]
    fn removed_device_clears_ui_state_and_selects_remaining_device() {
        let mut app = app();
        app.devices = HashMap::from([
            ("pods".into(), saved("Old")),
            ("other".into(), saved("Other")),
        ]);
        app.pending.insert("pods".into(), 2);
        app.connecting.insert("pods".into());
        app.forgetting.insert("pods".into());
        app.forget_confirm = Some("pods".into());
        app.nearby.insert("pods".into(), nearby("pods"));
        app.event_versions.insert(("pods".into(), 1), 3);
        app.receive_nearby(nearby("pods"));
        let _ = app.apply_devices(HashMap::from([("other".into(), saved("Other"))]));
        assert_eq!(app.selected_device.as_deref(), Some("other"));
        assert!(app.device_states.is_empty());
        assert!(app.pending.is_empty());
        assert!(app.event_versions.is_empty());
        assert!(app.connecting.is_empty());
        assert!(app.forgetting.is_empty());
        assert!(app.forget_confirm.is_none());
        assert!(app.bluetooth_state.connected_devices.is_empty());
        let _ = app.update(Message::ConnectFinished("pods".into(), Ok(())));
        assert!(!app.devices.contains_key("pods"));
        let _ = app.apply_devices(HashMap::new());
        assert!(app.selected_device.is_none());
    }
    #[test]
    fn forgetting_requires_confirmation_and_failure_keeps_device() {
        let mut app = app();
        app.devices.insert("pods".into(), saved("AirPods"));
        let _ = app.update(Message::ConfirmForget("pods".into()));
        assert!(app.forgetting.is_empty());
        let _ = app.update(Message::RequestForget("pods".into()));
        assert_eq!(app.forget_confirm.as_deref(), Some("pods"));
        let _ = app.update(Message::CancelForget);
        assert!(app.forget_confirm.is_none());
        let _ = app.update(Message::RequestForget("pods".into()));
        let _ = app.update(Message::ConfirmForget("pods".into()));
        assert!(app.forgetting.contains("pods"));
        let _ = app.update(Message::ForgetFinished(
            "pods".into(),
            Err("Permission denied".into()),
        ));
        assert!(app.devices.contains_key("pods"));
        assert!(app.errors["pods"].contains("Permission denied"));
        assert!(app.forgetting.is_empty());
    }
    fn nearby_app() -> App {
        let mut app = app();
        app.bluetooth_state.connected_devices.clear();
        app
    }
    fn nearby(id: &str) -> NearbyAirPods {
        NearbyAirPods {
            id: id.into(),
            name: "AirPods Pro".into(),
            connect_address: Some(id.into()),
            paired: true,
            connected: false,
            battery: vec![],
            lid_open: Some(true),
            lid_counter: Some(1),
            pairing: false,
        }
    }
    #[test]
    fn classic_inquiry_keeps_the_advertised_lid_and_battery_snapshot() {
        let mut app = nearby_app();
        let mut advertised = nearby("pods");
        advertised
            .battery
            .push(crate::bluetooth::aacp::BatteryInfo {
                component: crate::bluetooth::aacp::BatteryComponent::Case,
                level: 60,
                status: crate::bluetooth::aacp::BatteryStatus::NotCharging,
            });
        app.receive_nearby(advertised);
        let mut classic = nearby("pods");
        classic.lid_counter = None;
        classic.lid_open = None;
        app.receive_nearby(classic);
        assert_eq!(app.nearby["pods"].battery[0].level, 60);
        assert_eq!(app.nearby["pods"].lid_open, Some(true));
    }
    #[test]
    fn saved_headphones_stay_out_of_add_list_after_disconnecting() {
        let mut app = nearby_app();
        app.devices.insert("pods".into(), saved("AirPods Pro"));
        for connected in [true, false] {
            for lid_open in [Some(true), Some(false), None] {
                // Discovery can use either the saved MAC or a resolved LE identity.
                for id in ["pods", "private"] {
                    let mut device = nearby(id);
                    device.connect_address = Some("pods".into());
                    device.connected = connected;
                    device.lid_open = lid_open;
                    app.receive_nearby(device);
                    assert!(app.visible_nearby().is_empty());
                }
            }
        }
        let mut new_device = nearby("new");
        new_device.paired = false;
        app.receive_nearby(new_device);
        assert_eq!(app.visible_nearby().len(), 1);
        assert_eq!(app.visible_nearby()[0].id, "new");

        // Saving a discovered device immediately removes it from the add list.
        app.devices.insert("new".into(), saved("New AirPods"));
        assert!(app.visible_nearby().is_empty());
    }
    #[test]
    fn pairing_list_shows_audio_target_without_private_address_duplicates() {
        let mut app = nearby_app();
        for id in ["private-1", "private-2"] {
            let mut device = nearby(id);
            device.connect_address = None;
            device.paired = false;
            app.receive_nearby(device);
        }
        assert!(app.visible_nearby().is_empty());

        let mut device = nearby("classic");
        device.paired = false;
        device.pairing = true;
        device.lid_open = None;
        app.receive_nearby(device);
        let visible = app.visible_nearby();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "classic");

        // Two real audio targets with the same name remain separate devices.
        app.receive_nearby(nearby("second-classic"));
        assert_eq!(app.visible_nearby().len(), 2);
    }
    #[test]
    fn unresolved_private_address_never_starts_connection() {
        let mut app = nearby_app();
        let mut device = nearby("private");
        device.connect_address = None;
        app.receive_nearby(device);
        let _ = app.update(Message::ConnectNearby("private".into()));
        assert!(app.connecting.is_empty());
    }
    #[test]
    fn lost_device_keeps_an_active_connection_attempt() {
        let mut app = nearby_app();
        app.receive_nearby(nearby("pods"));
        app.connecting.insert("pods".into());
        let _ = app.update(Message::BluetoothMessage(BluetoothUIMessage::NearbyLost(
            "pods".into(),
        )));
        assert!(app.connecting.contains("pods"));
        let _ = app.update(Message::ConnectFinished("pods".into(), Ok(())));
        assert!(!app.connecting.contains("pods"));
    }
    #[test]
    fn failed_command_keeps_last_known_state() {
        let mut app = app();
        app.pending.insert("pods".into(), 1);
        let _ = app.update(Message::CommandFinished(
            "pods".into(),
            1,
            0,
            Command::Conversation(true),
            Err("Transport failed".into()),
        ));
        let DeviceState::AirPods(s) = &app.device_states["pods"];
        assert!(!s.conversation_awareness_enabled);
        assert!(app.errors["pods"].contains("Transport failed"));
        assert!(!app.pending.contains_key("pods"));
    }
    #[test]
    fn device_notification_wins_over_older_command_completion() {
        let mut app = app();
        app.pending.insert("pods".into(), 1);
        let _ = app.update(Message::BluetoothMessage(BluetoothUIMessage::AACPUIEvent(
            "pods".into(),
            AACPEvent::ControlCommand(crate::bluetooth::aacp::ControlCommandStatus {
                identifier: ControlCommandIdentifiers::ListeningMode,
                value: vec![4],
            }),
        )));
        let _ = app.update(Message::CommandFinished(
            "pods".into(),
            1,
            0,
            Command::Mode(AirPodsNoiseControlMode::NoiseCancellation),
            Ok(()),
        ));
        let DeviceState::AirPods(s) = &app.device_states["pods"];
        assert_eq!(s.noise_control_mode.to_byte(), 4);
    }
    #[test]
    fn disconnect_keeps_selection_and_ignores_late_completion() {
        let mut app = app();
        app.pending.insert("pods".into(), 1);
        let _ = app.update(Message::BluetoothMessage(
            BluetoothUIMessage::DeviceDisconnected("pods".into()),
        ));
        let _ = app.update(Message::CommandFinished(
            "pods".into(),
            1,
            0,
            Command::Conversation(true),
            Ok(()),
        ));
        assert_eq!(app.selected_device.as_deref(), Some("pods"));
        assert!(!app.device_states.contains_key("pods"));
        assert!(app.bluetooth_state.connected_devices.is_empty());
    }
    #[test]
    fn empty_rename_does_not_start_command() {
        let mut app = app();
        let _ = app.update(Message::Run("pods".into(), Command::Rename("  ".into())));
        assert!(app.pending.is_empty());
    }
    fn state() -> DeviceState {
        DeviceState::AirPods(AirPodsState {
            device_name: "AirPods Pro".into(),
            noise_control_mode: AirPodsNoiseControlMode::Transparency,
            conversation_awareness_enabled: false,
            personalized_volume_enabled: false,
            allow_off_mode: false,
            battery: vec![],
        })
    }
    #[test]
    fn stale_completion_cannot_finish_a_new_command() {
        let mut pending = HashMap::from([("pods".into(), 2)]);
        assert!(!finish_pending(&mut pending, "pods", 1));
        assert_eq!(pending.get("pods"), Some(&2));
        assert!(finish_pending(&mut pending, "pods", 2));
        assert!(!finish_pending(&mut pending, "pods", 2));
    }
    #[test]
    fn renaming_preserves_battery_and_listening_settings() {
        let mut device = state();
        apply_command(&mut device, &Command::Conversation(true));
        apply_command(&mut device, &Command::Rename("Work AirPods".into()));
        let DeviceState::AirPods(s) = device;
        assert_eq!(s.device_name, "Work AirPods");
        assert!(s.conversation_awareness_enabled);
        assert_eq!(
            s.noise_control_mode.to_byte(),
            AirPodsNoiseControlMode::Transparency.to_byte()
        );
    }
}

fn command_key(command: &Command) -> u8 {
    match command {
        Command::Mode(_) => ControlCommandIdentifiers::ListeningMode as u8,
        Command::Conversation(_) => ControlCommandIdentifiers::ConversationDetectConfig as u8,
        Command::Volume(_) => ControlCommandIdentifiers::AdaptiveVolumeConfig as u8,
        Command::AllowOff(_) => ControlCommandIdentifiers::AllowOffOption as u8,
        Command::Rename(_) => 255,
    }
}
