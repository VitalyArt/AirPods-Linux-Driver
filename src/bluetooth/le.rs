use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};
use crate::bluetooth::proximity::{self, NearbyAirPods};
use crate::devices::enums::{DeviceData, DeviceInformation, DeviceType};
use crate::ui::messages::BluetoothUIMessage;
use crate::ui::tray::MyTray;
use crate::utils::{ah, get_preferences_path};
use aes::Aes128;
use aes::cipher::{Array, BlockCipherDecrypt, KeyInit};
use bluer::{AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, Session};
use futures::StreamExt;
use log::{info, warn};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

pub const AIRPODS_UUID: &str = "74ec2172-0bad-4d01-8f77-997b2be0722a";
const EXPIRY: Duration = Duration::from_secs(25);

fn verify_rpa(address: Address, irk: &[u8; 16]) -> bool {
    let mut bytes = address.0;
    bytes.reverse();
    ah(irk, &bytes[3..6].try_into().unwrap()) == bytes[..3]
}

fn key(value: &str) -> Option<[u8; 16]> {
    hex::decode(value).ok()?.try_into().ok()
}

fn precise_battery(data: &[u8], key: &[u8; 16]) -> Vec<BatteryInfo> {
    let mut block = Array::from(<[u8; 16]>::try_from(&data[data.len() - 16..]).unwrap());
    Aes128::new(&Array::from(*key)).decrypt_block(&mut block);
    let left_primary = data[5] & 0x20 != 0;
    [
        (
            BatteryComponent::Left,
            block[if left_primary { 1 } else { 2 }],
        ),
        (
            BatteryComponent::Right,
            block[if left_primary { 2 } else { 1 }],
        ),
        (BatteryComponent::Case, block[3]),
    ]
    .into_iter()
    .filter_map(|(component, value)| {
        (value != 0xff && value & 0x7f <= 100).then_some(BatteryInfo {
            component,
            level: value & 0x7f,
            status: if value & 0x80 != 0 {
                BatteryStatus::Charging
            } else {
                BatteryStatus::NotCharging
            },
        })
    })
    .collect()
}

/// Standard BlueZ discovery also works without the experimental RegisterMonitor API.
/// Keep its stream alive; dropping it releases only our discovery session.
pub async fn start_le_monitor(
    tray: Option<ksni::Handle<MyTray>>,
    ui_tx: UnboundedSender<BluetoothUIMessage>,
) -> bluer::Result<()> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Auto,
            duplicate_data: true,
            ..Default::default()
        })
        .await?;
    let mut discovery = adapter.discover_devices_with_changes().await?;
    let mut seen: HashMap<Address, (Instant, String)> = HashMap::new();
    let mut processed = HashMap::<Address, Instant>::new();
    let mut auto_attempts = HashMap::<String, Instant>::new();
    let mut expiry = tokio::time::interval(Duration::from_secs(5));
    info!("Discovering AirPods advertisements and Bluetooth pairing devices");
    let _ = ui_tx.send(BluetoothUIMessage::DiscoveryReady);
    loop {
        let address = tokio::select! {
            event = discovery.next() => match event {
                Some(AdapterEvent::DeviceAdded(address)) => address,
                Some(AdapterEvent::DeviceRemoved(address)) => {
                    if let Some((_, id)) = seen.remove(&address)
                        && !seen.values().any(|(_, live_id)| *live_id == id) {
                        let _ = ui_tx.send(BluetoothUIMessage::NearbyLost(id));
                    }
                    processed.remove(&address);
                    continue;
                }
                Some(_) => continue,
                None => return Ok(()),
            },
            _ = expiry.tick() => {
                let now = Instant::now();
                let mut expired = Vec::new();
                seen.retain(|_, (time, id)| {
                    if now.duration_since(*time) > EXPIRY { expired.push(id.clone()); false } else { true }
                });
                for id in expired {
                    if !seen.values().any(|(_, live_id)| *live_id == id) {
                        let _ = ui_tx.send(BluetoothUIMessage::NearbyLost(id));
                    }
                }
                processed.retain(|_, time| now.duration_since(*time) < EXPIRY);
                auto_attempts.retain(|_, time| now.duration_since(*time) < Duration::from_secs(60));
                continue;
            }
        };
        let now = Instant::now();
        if let Some((time, _)) = seen.get_mut(&address) {
            *time = now;
        }
        if processed
            .get(&address)
            .is_some_and(|time| now.duration_since(*time) < Duration::from_millis(500))
        {
            continue;
        }
        processed.insert(address, now);
        let device = adapter.device(address)?;
        // Cached DeviceAdded objects without RSSI are not evidence of proximity.
        if device.rssi().await?.is_none() {
            continue;
        }
        let data = device.manufacturer_data().await?.unwrap_or_default();
        let ad = data.get(&76).and_then(|data| proximity::parse(data));
        let registered: HashMap<String, DeviceData> =
            crate::utils::device_store::read().unwrap_or_default();
        let matched = registered.iter().find(|(mac, entry)| {
            entry.type_ == DeviceType::AirPods
                && (mac.as_str() == address.to_string()
                    || ad
                        .as_ref()
                        .and_then(|ad| ad.pairing_address)
                        .is_some_and(|target| target.to_string() == **mac)
                    || matches!(&entry.information, Some(DeviceInformation::AirPods(info))
                    if key(&info.le_keys.irk).is_some_and(|irk| verify_rpa(address, &irk))))
        });
        let name = device.name().await?.unwrap_or_default();
        let uuids = device.uuids().await?.unwrap_or_default();
        let has_airpods_service = uuids.iter().any(|uuid| uuid.to_string() == AIRPODS_UUID);
        let random = device.address_type().await? == bluer::AddressType::LeRandom;
        let paired = device.is_paired().await?;
        // A random LE address alone is never a Bluetooth audio connection target.
        let audio_target =
            !random && (has_airpods_service || name.to_lowercase().contains("airpods"));
        if ad.is_none() && matched.is_none() && !audio_target {
            continue;
        }
        let connect_address = matched
            .map(|(mac, _)| mac.clone())
            .or_else(|| {
                ad.as_ref()
                    .and_then(|ad| ad.pairing_address)
                    .map(|address| address.to_string())
            })
            .or_else(|| audio_target.then(|| address.to_string()));
        let id = connect_address
            .clone()
            .unwrap_or_else(|| address.to_string());
        let mut nearby = NearbyAirPods {
            id: id.clone(),
            name: matched
                .map(|(_, d)| d.name.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| (!name.is_empty()).then_some(name))
                .unwrap_or_else(|| ad.as_ref().map_or("AirPods", |ad| ad.model).into()),
            connect_address,
            paired,
            connected: device.is_connected().await?,
            battery: ad.as_ref().map(|ad| ad.battery.clone()).unwrap_or_default(),
            lid_open: ad.as_ref().and_then(|ad| ad.lid_open),
            lid_counter: ad.as_ref().and_then(|ad| ad.lid_counter),
            pairing: ad.as_ref().is_some_and(|ad| ad.pairing) || (audio_target && !paired),
        };
        if let Some(mac) = &nearby.connect_address {
            let target = adapter.device(mac.parse().unwrap())?;
            nearby.paired = target.is_paired().await.unwrap_or(false);
            nearby.connected = target.is_connected().await.unwrap_or(false);
        }
        if let Some((_, entry)) = matched {
            if let (Some(ad), Some(DeviceInformation::AirPods(info))) = (&ad, &entry.information)
                && !ad.pairing
                && let Some(enc_key) = key(&info.le_keys.enc_key)
            {
                nearby.battery = precise_battery(&data[&76][..27], &enc_key);
            }
        }
        if let Some((_, previous_id)) = seen.insert(address, (now, id.clone()))
            && previous_id != id
            && !seen.values().any(|(_, live_id)| *live_id == previous_id)
        {
            let _ = ui_tx.send(BluetoothUIMessage::NearbyLost(previous_id));
        }
        if nearby.lid_open == Some(false) {
            let _ = ui_tx.send(BluetoothUIMessage::NearbyLost(id));
            continue;
        }
        if let (Some(handle), Some(mac)) = (&tray, &nearby.connect_address)
            && ad.as_ref().is_some_and(|ad| !ad.pairing)
        {
            let address = mac.parse().unwrap();
            let battery = nearby.battery.clone();
            handle
                .update(move |tray| {
                    if !tray.accepts(address) {
                        return;
                    }
                    let component = |kind| {
                        let value = battery.iter().find(|b| b.component == kind);
                        (
                            value.map(|b| b.level),
                            Some(value.map_or(BatteryStatus::Disconnected, |b| b.status)),
                        )
                    };
                    (tray.battery_l, tray.battery_l_status) = component(BatteryComponent::Left);
                    (tray.battery_r, tray.battery_r_status) = component(BatteryComponent::Right);
                    (tray.battery_c, tray.battery_c_status) = component(BatteryComponent::Case);
                })
                .await;
        }
        // Reconnect saved AirPods according to their autoConnect preference.
        if matched.is_some()
            && ad.as_ref().is_some_and(|ad| !ad.pairing)
            && data.get(&76).is_some_and(|data| data[10] == 0)
            && nearby.paired
            && !nearby.connected
            && let Some(mac) = nearby.connect_address.clone()
        {
            let preferences: HashMap<String, HashMap<String, bool>> =
                std::fs::read_to_string(get_preferences_path())
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
            if preferences
                .get(&mac)
                .and_then(|p| p.get("autoConnect"))
                .copied()
                .unwrap_or(true)
                && !auto_attempts.contains_key(&mac)
            {
                auto_attempts.insert(mac.clone(), now);
                crate::utils::device_store::spawn(mac.clone(), async move {
                    if let Err(error) = connect_airpods_impl(&mac, false).await {
                        warn!("Auto-connect failed: {error}");
                    }
                });
            }
        }
        let _ = ui_tx.send(BluetoothUIMessage::NearbyAirPods(nearby));
    }
}

/// Called only for an explicit audio target, or a saved address resolved with its IRK.
pub async fn connect_airpods(address: &str) -> Result<(), String> {
    connect_airpods_impl(address, true).await
}
async fn connect_airpods_impl(address: &str, allow_pairing: bool) -> Result<(), String> {
    let address: Address = address
        .parse()
        .map_err(|_| "Invalid Bluetooth address".to_owned())?;
    tokio::time::timeout(Duration::from_secs(45), async {
        let session = Session::new().await.map_err(|e| e.to_string())?;
        let adapter = session.default_adapter().await.map_err(|e| e.to_string())?;
        // The pairing advertisement can arrive before Classic inquiry creates
        // the target Device object. Our background Auto discovery resolves it.
        let deadline = Instant::now() + Duration::from_secs(12);
        while !adapter.device_addresses().await.map_err(|e| e.to_string())?.contains(&address) {
            if Instant::now() >= deadline { return Err("Bluetooth pairing device not found. Keep the case in pairing mode and try again.".into()); }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let device = adapter.device(address).map_err(|e| e.to_string())?;
        let _agent = session.register_agent(bluer::agent::Agent::default()).await.map_err(|e| e.to_string())?;
        if !device.is_paired().await.map_err(|e| e.to_string())? {
            if !allow_pairing { return Err("Pairing was removed. Pair again manually to reconnect.".into()); }
            device.pair().await.map_err(|e| format!("Pairing failed. Put the headphones in pairing mode and try again. {e}"))?;
        }
        device.set_trusted(true).await.map_err(|e| e.to_string())?;
        if device.connect().await.is_err() {
            // Keep the existing workaround for adapters where bluer Connect fails.
            let output = tokio::process::Command::new("bluetoothctl")
                .args(["connect", &address.to_string()]).kill_on_drop(true)
                .output().await.map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!("Connection failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
            }
        }
        if !device.is_connected().await.map_err(|e| e.to_string())? {
            return Err("The headphones did not connect. Open the case and try again.".into());
        }
        Ok(())
    }).await.unwrap_or_else(|_| Err("Connection timed out. Open the case or enable pairing mode and try again.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_rotating_address_with_matching_irk_only() {
        let irk = [0x12; 16];
        let prand = [0x56, 0x34, 0x52];
        let hash = ah(&irk, &prand);
        let address = Address([prand[2], prand[1], prand[0], hash[2], hash[1], hash[0]]);
        assert!(verify_rpa(address, &irk));
        assert!(!verify_rpa(address, &[0x13; 16]));
        assert!(key("short").is_none());
    }
}
