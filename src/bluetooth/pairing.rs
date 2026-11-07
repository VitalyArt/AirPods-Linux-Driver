//! System pairing is authoritative; advertisement expiry never removes saved devices.
use super::managers::DeviceManagers;
use crate::{
    devices::enums::{DeviceData, DeviceType},
    ui::messages::BluetoothUIMessage,
    utils::device_store,
};
use futures::StreamExt;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{RwLock, mpsc::UnboundedSender};

static PAIRING_OPERATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn reconcile(
    devices: &mut HashMap<String, DeviceData>,
    paired: &HashSet<String>,
    airpods: HashMap<String, String>,
) {
    devices.retain(|mac, _| paired.contains(mac));
    for (mac, name) in airpods {
        devices.entry(mac).or_insert(DeviceData {
            name,
            type_: DeviceType::AirPods,
            information: None,
        });
    }
}

/// Register metadata before protocol initialization can deliver information or keys.
pub async fn ensure_airpods(address: bluer::Address) -> Result<bool, String> {
    let _operation = PAIRING_OPERATION.lock().await;
    let session = bluer::Session::new().await.map_err(|e| e.to_string())?;
    let adapter = session.default_adapter().await.map_err(|e| e.to_string())?;
    let device = adapter.device(address).map_err(|e| e.to_string())?;
    if !device.is_paired().await.map_err(|e| e.to_string())? {
        return Ok(false);
    }
    let name = device
        .name()
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "AirPods".into());
    device_store::update(|devices| {
        devices.entry(address.to_string()).or_insert(DeviceData {
            name,
            type_: DeviceType::AirPods,
            information: None,
        });
    })?;
    Ok(true)
}

pub async fn monitor(
    tx: UnboundedSender<BluetoothUIMessage>,
    managers: Arc<RwLock<HashMap<String, DeviceManagers>>>,
    tray: Option<ksni::Handle<crate::ui::tray::MyTray>>,
) {
    loop {
        if let Err(e) = watch(&tx, &managers, &tray).await {
            log::warn!("Pairing synchronization unavailable: {e}");
            let _ = tx.send(BluetoothUIMessage::PairingError(format!(
                "Could not synchronize paired devices: {e}. Retrying…"
            )));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
async fn watch(
    tx: &UnboundedSender<BluetoothUIMessage>,
    managers: &Arc<RwLock<HashMap<String, DeviceManagers>>>,
    tray: &Option<ksni::Handle<crate::ui::tray::MyTray>>,
) -> Result<(), String> {
    let session = bluer::Session::new().await.map_err(|e| e.to_string())?;
    let adapter = session.default_adapter().await.map_err(|e| e.to_string())?;
    let mut events = adapter.events().await.map_err(|e| e.to_string())?;
    // Property changes (Paired, UUIDs) and BlueZ restarts are covered by complete
    // snapshots, including devices whose discovery happened before subscription.
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let mut previous = String::new();
    loop {
        tokio::select! {
            _ = interval.tick() => {},
            event = events.next() => { if event.is_none() { return Err("Bluetooth adapter disappeared".into()); } }
        }
        let _operation = PAIRING_OPERATION.lock().await;
        if !adapter.is_powered().await.map_err(|e| e.to_string())? {
            continue;
        }
        let mut paired = HashSet::new();
        let mut airpods = HashMap::new();
        for address in adapter
            .device_addresses()
            .await
            .map_err(|e| e.to_string())?
        {
            let device = adapter.device(address).map_err(|e| e.to_string())?;
            // Any failed read invalidates the whole snapshot. Never prune on partial data.
            if !device.is_paired().await.map_err(|e| e.to_string())? {
                continue;
            }
            let mac = address.to_string();
            paired.insert(mac.clone());
            if device
                .uuids()
                .await
                .map_err(|e| e.to_string())?
                .unwrap_or_default()
                .iter()
                .any(|u| u.to_string() == super::le::AIRPODS_UUID)
            {
                airpods.insert(
                    mac,
                    device
                        .name()
                        .await
                        .map_err(|e| e.to_string())?
                        .unwrap_or_else(|| "AirPods".into()),
                );
            }
        }
        if !adapter.is_powered().await.map_err(|e| e.to_string())? {
            continue;
        }
        let old = device_store::read()?;
        let devices = device_store::update(|devices| reconcile(devices, &paired, airpods))?;
        for mac in old.keys().filter(|mac| !devices.contains_key(*mac)) {
            cleanup(mac, managers, tray).await;
        }
        // Stable comparison independent of HashMap iteration order.
        let ordered: std::collections::BTreeMap<_, _> = devices.iter().collect();
        let json = serde_json::to_string(&ordered).map_err(|e| e.to_string())?;
        if json != previous {
            previous = json;
            let _ = tx.send(BluetoothUIMessage::DevicesChanged(devices));
        }
        let _ = tx.send(BluetoothUIMessage::PairingReady);
    }
}
async fn cleanup(
    mac: &str,
    managers: &Arc<RwLock<HashMap<String, DeviceManagers>>>,
    tray: &Option<ksni::Handle<crate::ui::tray::MyTray>>,
) {
    device_store::cancel(mac).await;
    if let Some(manager) = managers.write().await.remove(mac) {
        manager.shutdown().await;
    }
    if let (Some(tray), Ok(address)) = (tray, mac.parse()) {
        tray.update(move |tray| tray.disconnect(address)).await;
    }
    if let Err(e) = clear_preferences(mac) {
        log::warn!("Could not clear device preferences: {e}");
    }
}
fn clear_preferences(mac: &str) -> Result<(), String> {
    let path = crate::utils::get_preferences_path();
    let mut value: serde_json::Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if value.as_object_mut().and_then(|m| m.remove(mac)).is_some() {
        std::fs::write(path, serde_json::to_vec(&value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub async fn forget(
    mac: &str,
    managers: &Arc<RwLock<HashMap<String, DeviceManagers>>>,
    tray: &Option<ksni::Handle<crate::ui::tray::MyTray>>,
) -> Result<(), String> {
    let _operation = PAIRING_OPERATION.lock().await;
    let address = mac
        .parse()
        .map_err(|_| "Invalid Bluetooth address".to_string())?;
    tokio::time::timeout(Duration::from_secs(15), async {
        let session = bluer::Session::new().await.map_err(|e| e.to_string())?;
        let adapter = session.default_adapter().await.map_err(|e| e.to_string())?;
        if adapter
            .device_addresses()
            .await
            .map_err(|e| e.to_string())?
            .contains(&address)
        {
            adapter
                .remove_device(address)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "Bluetooth did not respond. Try again.".to_string())??;
    device_store::cancel(mac).await;
    device_store::update(|devices| {
        devices.remove(mac);
    })?;
    cleanup(mac, managers, tray).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_adds_paired_airpods_preserves_metadata_and_removes_forgotten_devices() {
        let mut devices = HashMap::from([
            (
                "saved".into(),
                DeviceData {
                    name: "Custom name".into(),
                    type_: DeviceType::AirPods,
                    information: None,
                },
            ),
            (
                "forgotten".into(),
                DeviceData {
                    name: "Old".into(),
                    type_: DeviceType::AirPods,
                    information: None,
                },
            ),
        ]);
        reconcile(
            &mut devices,
            &HashSet::from(["saved".into(), "new".into(), "mouse".into()]),
            HashMap::from([
                ("saved".into(), "AirPods".into()),
                ("new".into(), "New AirPods".into()),
            ]),
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices["saved"].name, "Custom name");
        assert_eq!(devices["new"].type_, DeviceType::AirPods);
        reconcile(
            &mut devices,
            &HashSet::from(["saved".into(), "new".into()]),
            HashMap::new(),
        );
        assert_eq!(
            devices.len(),
            2,
            "UUIDs not yet available must not remove paired devices"
        );
    }
}
