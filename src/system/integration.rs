//! Desktop-facing API and a best-effort BlueZ aggregate battery provider.
use crate::bluetooth::{
    aacp::{BatteryComponent, BatteryInfo, BatteryStatus, ControlCommandIdentifiers},
    managers::DeviceManagers,
};
use dbus::{
    Path,
    arg::{PropMap, Variant},
    blocking::Connection,
    channel::{MatchingReceiver, Sender},
    message::MatchRule,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

const ROOT: &str = "/com/vitalyart/AirpodsDriver";
const API: &str = "com.vitalyart.AirpodsDriver1";
const PROVIDER: &str = "org.bluez.BatteryProvider1";
type Objects = HashMap<Path<'static>, HashMap<String, PropMap>>;
type Managers = Arc<RwLock<HashMap<String, DeviceManagers>>>;

fn percentage(readings: &[BatteryInfo]) -> Option<u8> {
    readings
        .iter()
        .filter(|b| {
            b.component != BatteryComponent::Case
                && b.status != BatteryStatus::Disconnected
                && b.level <= 100
        })
        .map(|b| b.level)
        .min()
}

pub fn start(managers: Managers) {
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        if let Err(error) = serve(managers, runtime, true) {
            log::warn!("Desktop D-Bus integration unavailable: {error}");
        }
    });
}

fn serve(
    managers: Managers,
    runtime: tokio::runtime::Handle,
    publish_battery: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = Connection::new_session()?;
    if session.request_name("com.vitalyart.AirpodsDriver", false, false, true)?
        != dbus::blocking::stdintf::org_freedesktop_dbus::RequestNameReply::PrimaryOwner
    {
        return Err("Desktop API name is already owned".into());
    }
    let command_managers = managers.clone();
    let command_runtime = runtime.clone();
    session.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |message, connection| {
            if message.path().as_deref() != Some(ROOT) {
                return true;
            }
            let result = match (message.interface().as_deref(), message.member().as_deref()) {
                (Some(API), Some("GetDevices")) => {
                    let devices = command_runtime.block_on(snapshot(&command_managers));
                    Ok(message.method_return().append1(devices.to_string()))
                }
                (Some(API), Some("SetListeningMode")) => match message.read2::<String, u8>() {
                    Ok((address, mode)) if (1..=4).contains(&mode) => {
                        let result = command_runtime.block_on(async {
                            let manager = command_managers
                                .read()
                                .await
                                .get(&address)
                                .and_then(|m| m.get_aacp())
                                .ok_or("Unknown device".to_string())?;
                            if manager.state.lock().await.sender.is_none() {
                                return Err("Device is disconnected".to_string());
                            }
                            manager
                                .send_control_command(
                                    ControlCommandIdentifiers::ListeningMode,
                                    &[mode],
                                )
                                .await
                                .map_err(|e| e.to_string())
                        });
                        result.map(|_| message.method_return())
                    }
                    _ => Err("Expected device address and mode byte 1..4".into()),
                },
                (Some("org.freedesktop.DBus.Introspectable"), Some("Introspect")) => {
                    Ok(message.method_return().append1(INTROSPECTION))
                }
                _ => Err("Unknown method".into()),
            };
            let reply = result.unwrap_or_else(|error: String| {
                message.error(
                    &"org.freedesktop.DBus.Error.Failed".into(),
                    &std::ffi::CString::new(error.replace('\0', "")).unwrap(),
                )
            });
            let _ = connection.send(reply);
            true
        }),
    );
    let objects = Arc::new(std::sync::Mutex::new(Objects::new()));
    let system = if publish_battery {
        Connection::new_system()
            .map_err(|e| log::warn!("System battery integration unavailable: {e}"))
            .ok()
    } else {
        None
    };
    if let Some(connection) = &system {
        let objects = objects.clone();
        connection.start_receive(
            MatchRule::new_method_call(),
            Box::new(move |message, connection| {
                if message.path().as_deref() == Some(ROOT)
                    && message.interface().as_deref() == Some("org.freedesktop.DBus.ObjectManager")
                    && message.member().as_deref() == Some("GetManagedObjects")
                {
                    let _ =
                        connection.send(message.method_return().append1(&*objects.lock().unwrap()));
                }
                true
            }),
        );
    }
    let mut registered = HashMap::<String, String>::new();
    let mut previous = String::new();
    let mut tick = 0u64;
    loop {
        session.process(Duration::from_millis(100))?;
        if let Some(connection) = &system {
            connection.process(Duration::ZERO)?;
        }
        tick += 1;
        if tick % 10 != 0 {
            continue;
        }
        let data = runtime.block_on(snapshot(&managers)).to_string();
        if data != previous {
            let _ = session.send(
                dbus::Message::new_signal(ROOT, API, "DevicesChanged")
                    .unwrap()
                    .append1(data.clone()),
            );
            previous = data;
        }
        let Some(connection) = &system else {
            continue;
        };
        let proxy = connection.with_proxy("org.bluez", "/", Duration::from_secs(2));
        let Ok((bluez,)) = proxy.method_call::<(Objects,), _, _, _>(
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
            (),
        ) else {
            registered.clear();
            continue;
        };
        let owner = connection
            .with_proxy(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                Duration::from_secs(2),
            )
            .method_call::<(String,), _, _, _>(
                "org.freedesktop.DBus",
                "GetNameOwner",
                ("org.bluez",),
            )
            .map(|v| v.0)
            .unwrap_or_default();
        let mut next = Objects::new();
        for (adapter, interfaces) in &bluez {
            if !interfaces.contains_key("org.bluez.BatteryProviderManager1") {
                continue;
            }
            if registered.get(&adapter.to_string()) != Some(&owner) {
                if tick % 300 != 10 {
                    continue;
                }
                let result: Result<(), _> = connection
                    .with_proxy("org.bluez", adapter.clone(), Duration::from_secs(2))
                    .method_call(
                        "org.bluez.BatteryProviderManager1",
                        "RegisterBatteryProvider",
                        (Path::new(ROOT).unwrap(),),
                    );
                match result {
                    Ok(()) => {
                        registered.insert(adapter.to_string(), owner.clone());
                    }
                    Err(e) => {
                        if tick % 300 == 10 {
                            log::warn!("Cannot publish AirPods battery on {adapter}: {e}");
                        }
                        continue;
                    }
                }
            }
            let live = runtime.block_on(async {
                managers
                    .read()
                    .await
                    .iter()
                    .filter_map(|(a, m)| m.get_aacp().map(|m| (a.clone(), m)))
                    .collect::<Vec<_>>()
            });
            for (address, manager) in live {
                let device = format!("{adapter}/dev_{}", address.replace(':', "_"));
                let connected = bluez
                    .get(&Path::new(device.clone()).unwrap())
                    .and_then(|i| i.get("org.bluez.Device1"))
                    .and_then(|p| p.get("Connected"))
                    .and_then(|v| v.0.as_i64())
                    .is_some_and(|v| v != 0);
                if !connected {
                    continue;
                }
                let level = runtime
                    .block_on(async { percentage(&manager.state.lock().await.battery_info) });
                if let Some(level) = level {
                    let mut props = PropMap::new();
                    props.insert(
                        "Device".into(),
                        Variant(Box::new(Path::new(device).unwrap())),
                    );
                    props.insert("Percentage".into(), Variant(Box::new(level)));
                    props.insert(
                        "Source".into(),
                        Variant(Box::new("Airpods Driver".to_string())),
                    );
                    next.insert(
                        Path::new(format!(
                            "{ROOT}/{}/dev_{}",
                            adapter.rsplit('/').next().unwrap(),
                            address.replace(':', "_")
                        ))
                        .unwrap(),
                        HashMap::from([(PROVIDER.into(), props)]),
                    );
                }
            }
        }
        let mut current = objects.lock().unwrap();
        for path in current.keys() {
            if !next.contains_key(path) {
                let _ = connection.send(
                    dbus::Message::new_signal(
                        ROOT,
                        "org.freedesktop.DBus.ObjectManager",
                        "InterfacesRemoved",
                    )
                    .unwrap()
                    .append2(path.clone(), vec![PROVIDER.to_string()]),
                );
            }
        }
        for (path, interfaces) in &next {
            if !current.contains_key(path) {
                let _ = connection.send(
                    dbus::Message::new_signal(
                        ROOT,
                        "org.freedesktop.DBus.ObjectManager",
                        "InterfacesAdded",
                    )
                    .unwrap()
                    .append2(path.clone(), interfaces),
                );
            } else {
                let _ = connection.send(
                    dbus::Message::new_signal(
                        path.to_string(),
                        "org.freedesktop.DBus.Properties",
                        "PropertiesChanged",
                    )
                    .unwrap()
                    .append3(
                        PROVIDER,
                        &interfaces[PROVIDER],
                        Vec::<String>::new(),
                    ),
                );
            }
        }
        *current = next;
    }
}

async fn snapshot(managers: &Managers) -> serde_json::Value {
    let live = managers
        .read()
        .await
        .iter()
        .filter_map(|(address, m)| m.get_aacp().map(|m| (address.clone(), m)))
        .collect::<Vec<_>>();
    let mut devices = serde_json::Map::new();
    for (address, manager) in live {
        let state = manager.state.lock().await;
        let mode = state
            .control_command_status_list
            .iter()
            .find(|s| s.identifier == ControlCommandIdentifiers::ListeningMode)
            .and_then(|s| s.value.first())
            .copied();
        devices.insert(address, serde_json::json!({"connected": state.sender.is_some(), "listening_mode": mode, "batteries": state.battery_info.iter().filter(|b| b.level <= 100 && b.status != BatteryStatus::Disconnected).map(|b| serde_json::json!({"component": b.component as u8, "percentage": b.level, "charging": b.status == BatteryStatus::Charging})).collect::<Vec<_>>() }));
    }
    serde_json::Value::Object(devices)
}

const INTROSPECTION: &str = r#"<node><interface name="com.vitalyart.AirpodsDriver1"><method name="GetDevices"><arg name="devices_json" type="s" direction="out"/></method><method name="SetListeningMode"><arg name="address" type="s" direction="in"/><arg name="mode" type="y" direction="in"/></method><signal name="DevicesChanged"><arg name="devices_json" type="s"/></signal></interface><interface name="org.freedesktop.DBus.Introspectable"><method name="Introspect"><arg type="s" direction="out"/></method></interface></node>"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "run inside a private dbus-run-session"]
    fn session_api_roundtrip() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return;
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        std::thread::spawn(move || {
            let _ = serve(Arc::new(RwLock::new(HashMap::new())), handle, false);
        });
        let connection = Connection::new_session().unwrap();
        let proxy =
            connection.with_proxy("com.vitalyart.AirpodsDriver", ROOT, Duration::from_secs(2));
        let mut result = None;
        for _ in 0..50 {
            if let Ok((json,)) = proxy.method_call::<(String,), _, _, _>(API, "GetDevices", ()) {
                result = Some(json);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(result.as_deref(), Some("{}"));
        let (xml,): (String,) = proxy
            .method_call("org.freedesktop.DBus.Introspectable", "Introspect", ())
            .unwrap();
        assert!(xml.contains("SetListeningMode"));
        assert!(
            proxy
                .method_call::<(), _, _, _>(API, "SetListeningMode", ("unknown", 2u8))
                .is_err()
        );
        assert!(
            proxy
                .method_call::<(), _, _, _>(API, "SetListeningMode", ("unknown", 255u8))
                .is_err()
        );
    }
    #[test]
    fn aggregate_excludes_case_unknown_and_disconnected() {
        let b = |component, level, status| BatteryInfo {
            component,
            level,
            status,
        };
        assert_eq!(percentage(&[]), None);
        assert_eq!(
            percentage(&[b(BatteryComponent::Case, 5, BatteryStatus::Charging)]),
            None
        );
        assert_eq!(
            percentage(&[
                b(BatteryComponent::Left, 80, BatteryStatus::NotCharging),
                b(BatteryComponent::Right, 40, BatteryStatus::Charging),
                b(BatteryComponent::Case, 5, BatteryStatus::Charging)
            ]),
            Some(40)
        );
        assert_eq!(
            percentage(&[
                b(BatteryComponent::Left, 255, BatteryStatus::NotCharging),
                b(BatteryComponent::Right, 0, BatteryStatus::Disconnected)
            ]),
            None
        );
    }
}
