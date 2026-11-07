//! Serialized device metadata updates. BlueZ owns pairing; this file only caches metadata.
use crate::devices::enums::DeviceData;
use std::{collections::HashMap, future::Future, sync::Mutex};
use tokio::task::AbortHandle;

type Devices = HashMap<String, DeviceData>;
static STORE: Mutex<()> = Mutex::new(());
static TASKS: Mutex<Option<HashMap<String, Vec<AbortHandle>>>> = Mutex::new(None);

pub fn read() -> Result<Devices, String> {
    let _guard = STORE.lock().unwrap();
    read_at(&crate::utils::get_devices_path())
}
fn read_at(path: &std::path::Path) -> Result<Devices, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let records: HashMap<String, serde_json::Value> =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            records
                .into_iter()
                .filter_map(|(mac, record)| {
                    // Ignore removed integrations without hiding malformed AirPods metadata.
                    if record
                        .get("type_")
                        .and_then(|value| value.as_str())
                        .is_some_and(|kind| kind != "AirPods")
                    {
                        return None;
                    }
                    Some(
                        serde_json::from_value(record)
                            .map(|device| (mac, device))
                            .map_err(|e| e.to_string()),
                    )
                })
                .collect()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn update(f: impl FnOnce(&mut Devices)) -> Result<Devices, String> {
    update_at(&crate::utils::get_devices_path(), f)
}
fn update_at(path: &std::path::Path, f: impl FnOnce(&mut Devices)) -> Result<Devices, String> {
    let _guard = STORE.lock().unwrap();
    let mut devices = read_at(path)?;
    let before = serde_json::to_vec(&devices).map_err(|e| e.to_string())?;
    f(&mut devices);
    let bytes = serde_json::to_vec(&devices).map_err(|e| e.to_string())?;
    if bytes != before {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
    }
    Ok(devices)
}
/// Device-specific workers are explicitly cancelled when pairing is removed.
pub fn spawn(mac: String, future: impl Future<Output = ()> + Send + 'static) {
    let mut registry = TASKS.lock().unwrap();
    let handles = registry
        .get_or_insert_with(HashMap::new)
        .entry(mac)
        .or_default();
    handles.retain(|h| !h.is_finished());
    handles.push(tokio::spawn(future).abort_handle());
}
pub fn track(mac: String, handle: AbortHandle) {
    let mut registry = TASKS.lock().unwrap();
    let handles = registry
        .get_or_insert_with(HashMap::new)
        .entry(mac)
        .or_default();
    handles.retain(|h| !h.is_finished());
    handles.push(handle);
}
pub async fn cancel(mac: &str) {
    loop {
        let handles = TASKS
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|m| m.remove(mac))
            .unwrap_or_default();
        if handles.is_empty() {
            break;
        }
        for handle in &handles {
            handle.abort();
        }
        // Wait for destructors before dropping transport managers. An initializer
        // may have registered child workers just before it observed cancellation.
        while handles.iter().any(|h| !h.is_finished()) {
            tokio::task::yield_now().await;
        }
    }
}

pub fn is_running(mac: &str) -> bool {
    TASKS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(mac))
        .is_some_and(|tasks| tasks.iter().any(|t| !t.is_finished()))
}
pub async fn run<T: Send + 'static>(
    mac: String,
    future: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    spawn(mac, async move {
        let _ = tx.send(future.await);
    });
    rx.await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::enums::DeviceType;
    fn device(name: &str) -> DeviceData {
        DeviceData {
            name: name.into(),
            type_: DeviceType::AirPods,
            information: None,
        }
    }
    #[test]
    fn serialized_updates_preserve_other_devices_and_late_metadata_cannot_resurrect() {
        let path = std::env::temp_dir().join(format!(
            "airpods-driver-store-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    update_at(path, |devices| {
                        devices.insert(index.to_string(), device("AirPods"));
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(read_at(&path).unwrap().len(), 8);
        update_at(&path, |devices| {
            devices.remove("0");
        })
        .unwrap();
        let devices = update_at(&path, |devices| {
            if let Some(device) = devices.get_mut("0") {
                device.name = "Late packet".into();
            }
        })
        .unwrap();
        assert_eq!(devices.len(), 7);
        assert!(!devices.contains_key("0"));
        // Invalid metadata must surface as an error instead of overwriting the file.
        std::fs::write(&path, b"invalid JSON").unwrap();
        assert!(
            update_at(&path, |devices| {
                devices.clear();
            })
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid JSON");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn old_unsupported_devices_do_not_hide_airpods_or_erase_their_keys() {
        use crate::devices::{airpods::AirPodsInformation, enums::DeviceInformation};
        let path = std::env::temp_dir().join(format!(
            "airpods-driver-migration-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut airpods = device("My AirPods");
        let mut information = AirPodsInformation::default();
        information.le_keys.irk = "saved-identity-key".into();
        information.le_keys.enc_key = "saved-encryption-key".into();
        airpods.information = Some(DeviceInformation::AirPods(information));
        let expected = serde_json::to_value(&airpods).unwrap();
        let records = serde_json::json!({
            "pods": expected,
            "old": {
                "name": "Old headphones",
                "type_": "Nothing",
                "information": {"kind": "Nothing", "data": {"serial_number": "old"}}
            }
        });
        std::fs::write(&path, serde_json::to_vec(&records).unwrap()).unwrap();
        let loaded = read_at(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(serde_json::to_value(&loaded["pods"]).unwrap(), expected);
        update_at(&path, |devices| {
            devices.insert("new".into(), device("New AirPods"));
        })
        .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("old").is_none());
        assert_eq!(saved["pods"], expected);
        // Corrupt supported records must still fail instead of silently losing them.
        std::fs::write(&path, br#"{"pods":{"type_":"AirPods"}}"#).unwrap();
        assert!(read_at(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn forgetting_cancels_running_device_workers() {
        let mac = "cancel-test-device".to_string();
        let (started, rx) = tokio::sync::oneshot::channel();
        let worker_mac = mac.clone();
        let worker = tokio::spawn(async move {
            run(worker_mac, async move {
                let _ = started.send(());
                std::future::pending::<()>().await;
            })
            .await
        });
        rx.await.unwrap();
        assert!(is_running(&mac));
        cancel(&mac).await;
        assert!(worker.await.unwrap().is_none());
        assert!(!is_running(&mac));
    }
}
