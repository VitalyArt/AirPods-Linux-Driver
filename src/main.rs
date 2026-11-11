mod bluetooth;
mod devices;
mod system;
use system::single_instance;
mod ui;
mod utils;

use crate::bluetooth::discovery::find_connected_airpods;
use crate::bluetooth::le::start_le_monitor;
use crate::bluetooth::managers::DeviceManagers;
use crate::ui::messages::BluetoothUIMessage;
use crate::ui::tray::MyTray;
use bluer::Address;
use clap::Parser;
use dbus::arg::{RefArg, Variant};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::message::MatchRule;
use devices::airpods::AirPodsDevice;
use ksni::TrayMethods;
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc::unbounded_channel;

#[derive(Parser)]
struct Args {
    #[arg(long, short = 'd', help = "Enable debug logging")]
    debug: bool,
    #[arg(
        long,
        help = "Disable system tray, useful if your environment doesn't support AppIndicator or StatusNotifier"
    )]
    no_tray: bool,
    #[arg(long, help = "Start the application minimized to tray")]
    start_minimized: bool,
    #[arg(
        long,
        help = "Enable Bluetooth LE debug logging. Only use when absolutely necessary; this produces a lot of logs."
    )]
    le_debug: bool,
    #[arg(long, short = 'v', help = "Show application version and exit")]
    version: bool,
}

fn main() -> iced::Result {
    let args = Args::parse();

    if args.version {
        println!(
            "You are running Airpods Driver version {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    let log_level = if args.debug { "debug" } else { "info" };
    // let wayland_display = env::var("WAYLAND_DISPLAY").is_ok();
    // if wayland_display && env::var("WGPU_BACKEND").is_err() {
    //     unsafe { env::set_var("WGPU_BACKEND", "gl") };
    // }
    if env::var("RUST_LOG").is_err() {
        unsafe {
            env::set_var(
                "RUST_LOG",
                log_level.to_owned()
                    + &format!(
                        ",zbus=warn,winit=warn,tracing=warn,iced_wgpu=warn,wgpu_hal=warn,wgpu_core=warn,cosmic_text=warn,naga=warn,iced_winit=warn,airpods_driver::bluetooth::le={}",
                        if args.le_debug { "debug" } else { "info" }
                    ),
            )
        };
    }
    env_logger::init();

    let _instance = match single_instance::acquire() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            info!("Airpods Driver is already running; exiting.");
            return Ok(());
        }
        Err(error) => {
            eprintln!("Cannot acquire the Airpods Driver instance lock: {error}");
            std::process::exit(1);
        }
    };

    let (ui_tx, ui_rx) = unbounded_channel::<BluetoothUIMessage>();

    let device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Load stem_control initial value from settings JSON, then apply CLI override.
    if args.no_tray {
        if let Err(error) = system::autostart::initialize() {
            warn!("Could not initialize autostart: {error}");
        }
        // Run headless without UI
        info!("Running in headless mode (no GUI)");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async_main(ui_tx, device_managers)).unwrap();
        Ok(())
    } else {
        // Run with UI
        let device_managers_clone = device_managers.clone();
        let failure_ui_tx = ui_tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            if let Err(error) = rt.block_on(async_main(ui_tx, device_managers_clone)) {
                warn!("Bluetooth is unavailable: {error}");
                let _ = failure_ui_tx.send(BluetoothUIMessage::DiscoveryError(format!(
                    "Bluetooth is unavailable: {error}"
                )));
            }
        });

        ui::window::start_ui(ui_rx, args.start_minimized, device_managers)
    }
}

async fn async_main(
    ui_tx: tokio::sync::mpsc::UnboundedSender<BluetoothUIMessage>,
    device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
) -> bluer::Result<()> {
    let args = Args::parse();
    system::integration::start(device_managers.clone());

    let tray_handle = if args.no_tray {
        None
    } else {
        let text_mode = std::fs::read_to_string(crate::utils::get_app_settings_path())
            .ok()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|settings| settings.get("tray_text_mode").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        let tray = MyTray {
            text_mode,
            ui_tx: Some(ui_tx.clone()),
            ..Default::default()
        };
        match tray.spawn().await {
            Ok(handle) => {
                let _ = ui_tx.send(BluetoothUIMessage::TrayReady(crate::ui::tray::TrayHandle(
                    handle.clone(),
                )));
                Some(handle)
            }
            Err(error) => {
                warn!("System tray is unavailable: {error}");
                None
            }
        }
    };

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    tokio::spawn(bluetooth::pairing::monitor(
        ui_tx.clone(),
        device_managers.clone(),
        tray_handle.clone(),
    ));

    let le_tray_clone = tray_handle.clone();
    let le_ui_tx = ui_tx.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = start_le_monitor(le_tray_clone.clone(), le_ui_tx.clone()).await {
                log::warn!("AirPods discovery error: {e}");
                let _ = le_ui_tx.send(BluetoothUIMessage::DiscoveryError(format!(
                    "Nearby discovery is unavailable: {e}. Retrying…"
                )));
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    info!("Listening for new connections.");

    info!("Checking for connected devices...");
    match find_connected_airpods(&adapter).await {
        Ok(device) => {
            let name = device
                .name()
                .await?
                .unwrap_or_else(|| "Unknown".to_string());
            info!("Found connected AirPods: {}, initializing.", name);
            let address = device.address();
            let registered = match bluetooth::pairing::ensure_airpods(address).await {
                Ok(paired) => paired,
                Err(e) => {
                    warn!("Could not register paired AirPods: {e}");
                    false
                }
            };
            let tray = tray_handle.clone();
            let tx = ui_tx.clone();
            let airpods_device = if registered {
                crate::utils::device_store::run(address.to_string(), async move {
                    AirPodsDevice::new(address, tray, tx).await
                })
                .await
            } else {
                None
            };
            if let Some(airpods_device) = airpods_device {
                let mut managers = device_managers.write().await;
                if !crate::utils::device_store::read()
                    .is_ok_and(|d| d.contains_key(&address.to_string()))
                {
                    airpods_device.aacp_manager.shutdown().await;
                } else {
                    let dev_managers =
                        DeviceManagers::with_aacp(airpods_device.aacp_manager.clone());
                    managers
                        .entry(device.address().to_string())
                        .or_insert(dev_managers)
                        .set_aacp(airpods_device.aacp_manager);
                    drop(managers);
                    if let Err(e) = ui_tx.send(BluetoothUIMessage::DeviceConnected(
                        device.address().to_string(),
                    )) {
                        warn!("Failed to send DeviceConnected UI message: {:?}", e);
                    }
                }
            }
        }
        Err(_) => {
            info!("No connected AirPods found.");
        }
    }

    let mut initialized: HashSet<String> = device_managers.read().await.keys().cloned().collect();
    let conn = Connection::new_system()?;
    let rule = MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged");
    conn.add_match(rule, move |_: (), conn, msg| {
        let Some(path) = msg.path() else {
            return true;
        };
        if !path.contains("/org/bluez/hci") || !path.contains("/dev_") {
            return true;
        }
        // debug!("PropertiesChanged signal for path: {}", path);
        let Ok((iface, changed, _)) =
            msg.read3::<String, HashMap<String, Variant<Box<dyn RefArg>>>, Vec<String>>()
        else {
            return true;
        };
        if iface != "org.bluez.Device1" {
            return true;
        }
        // First pairing may emit Connected before SDP has populated UUIDs.
        // Reconsider the device when its services become available as well.
        if !["Connected", "ServicesResolved", "UUIDs"]
            .iter()
            .any(|key| changed.contains_key(*key))
        {
            return true;
        }
        let proxy = conn.with_proxy("org.bluez", path, std::time::Duration::from_millis(5000));
        let connected = changed
            .get("Connected")
            .and_then(|v| v.0.as_ref().as_u64())
            .map(|v| v != 0)
            .or_else(|| proxy.get::<bool>("org.bluez.Device1", "Connected").ok());
        let Some(connected) = connected else {
            return true;
        };
        let Ok(addr_str) = proxy.get::<String>("org.bluez.Device1", "Address") else {
            return true;
        };
        let Ok(addr) = addr_str.parse::<Address>() else {
            return true;
        };
        if !connected {
            initialized.remove(&addr_str);
            if let Some(handle) = tray_handle.clone() {
                tokio::spawn(async move {
                    handle.update(move |tray| tray.disconnect(addr)).await;
                });
            }
            if let Err(e) = ui_tx.send(BluetoothUIMessage::DeviceDisconnected(addr_str.clone())) {
                warn!("Failed to send DeviceConnected UI message: {:?}", e);
            }
            return true;
        }
        if initialized.contains(&addr_str) && crate::utils::device_store::is_running(&addr_str) {
            return true;
        }
        let Ok(uuids) = proxy.get::<Vec<String>>("org.bluez.Device1", "UUIDs") else {
            return true;
        };
        let target_uuid = "74ec2172-0bad-4d01-8f77-997b2be0722a";

        if !uuids.iter().any(|u| u.to_lowercase() == target_uuid) {
            return true;
        }
        if !proxy
            .get::<bool>("org.bluez.Device1", "Paired")
            .unwrap_or(false)
        {
            return true;
        }
        initialized.insert(addr_str.clone());
        let name = proxy
            .get::<String>("org.bluez.Device1", "Name")
            .unwrap_or_else(|_| "Unknown".to_string());
        info!("AirPods connected: {}, initializing", name);
        let handle_clone = tray_handle.clone();
        let ui_tx_clone = ui_tx.clone();
        let device_managers = device_managers.clone();
        crate::utils::device_store::spawn(addr_str.clone(), async move {
            match bluetooth::pairing::ensure_airpods(addr).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => {
                    warn!("Could not register paired AirPods: {e}");
                    return;
                }
            }
            let airpods_device = AirPodsDevice::new(addr, handle_clone, ui_tx_clone.clone()).await;
            let mut managers = device_managers.write().await;
            let dev_managers = DeviceManagers::with_aacp(airpods_device.aacp_manager.clone());
            managers
                .entry(addr_str.clone())
                .or_insert(dev_managers)
                .set_aacp(airpods_device.aacp_manager);
            drop(managers);
            if let Err(e) = ui_tx_clone.send(BluetoothUIMessage::DeviceConnected(addr_str.clone()))
            {
                warn!("Failed to send DeviceConnected UI message: {:?}", e);
            }
        });
        true
    })?;

    info!("Listening for Bluetooth connections via D-Bus...");
    loop {
        conn.process(std::time::Duration::from_millis(1000))?;
    }
}
