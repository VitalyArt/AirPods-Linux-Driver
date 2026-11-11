//! Low-battery alerts from connected-device readings only.
use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};
use dbus::arg::{RefArg, Variant};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const LOW_LEVEL: u8 = 20;
const RESET_LEVEL: u8 = 30;

#[derive(Default)]
struct BatteryAlerts {
    // Keep state across reconnects for the lifetime of the application.
    notified: HashMap<(String, u8), bool>,
}

impl BatteryAlerts {
    fn update(&mut self, address: &str, readings: &[BatteryInfo]) -> Vec<String> {
        let mut alerts = Vec::new();
        for reading in readings {
            if reading.status == BatteryStatus::Disconnected || reading.level > 100 {
                continue;
            }
            let notified = self.notified
                .entry((address.to_owned(), reading.component as u8))
                .or_default();
            if reading.level >= RESET_LEVEL {
                *notified = false;
            }
            if reading.status == BatteryStatus::NotCharging
                && reading.level <= LOW_LEVEL && !*notified
            {
                *notified = true;
                let label = match reading.component {
                    BatteryComponent::Left => "Left earbud",
                    BatteryComponent::Right => "Right earbud",
                    BatteryComponent::Case => "Charging case",
                    BatteryComponent::Headphone => "Headphones",
                };
                alerts.push(format!("{label}: {}%", reading.level));
            }
        }
        alerts
    }
}

pub async fn notify_low_battery(address: &str, readings: &[BatteryInfo]) {
    static ALERTS: OnceLock<Mutex<BatteryAlerts>> = OnceLock::new();
    let lines = ALERTS.get_or_init(Default::default).lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .update(address, readings);
    if lines.is_empty() {
        return;
    }
    // The notification service may be absent; never interrupt Bluetooth handling.
    let body = lines.join("\n");
    let result = tokio::task::spawn_blocking(move || -> Result<(), dbus::Error> {
        let connection = dbus::blocking::Connection::new_session()?;
        let proxy = connection.with_proxy(
            "org.freedesktop.Notifications", "/org/freedesktop/Notifications",
            Duration::from_secs(2),
        );
        let mut hints: HashMap<String, Variant<Box<dyn RefArg>>> = HashMap::new();
        hints.insert("desktop-entry".into(), Variant(Box::new(
            "com.vitalyart.airpods-driver".to_owned(),
        )));
        let _: (u32,) = proxy.method_call(
            "org.freedesktop.Notifications", "Notify",
            ("Airpods Driver", 0u32, "com.vitalyart.airpods-driver",
             "AirPods battery is low", body, Vec::<String>::new(), hints, -1i32),
        )?;
        Ok(())
    }).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => log::warn!("Could not show battery notification: {error}"),
        Err(error) => log::warn!("Battery notification task failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(component: BatteryComponent, level: u8, status: BatteryStatus) -> BatteryInfo {
        BatteryInfo { component, level, status }
    }

    #[test]
    fn alerts_once_and_rearms_only_after_recovery() {
        let mut alerts = BatteryAlerts::default();
        let mut check = |level, status| alerts.update("device", &[
            reading(BatteryComponent::Left, level, status),
        ]).len();
        assert_eq!(check(21, BatteryStatus::NotCharging), 0);
        assert_eq!(check(20, BatteryStatus::NotCharging), 1);
        assert_eq!(check(19, BatteryStatus::NotCharging), 0);
        assert_eq!(check(21, BatteryStatus::NotCharging), 0);
        assert_eq!(check(20, BatteryStatus::NotCharging), 0);
        assert_eq!(check(25, BatteryStatus::Charging), 0);
        assert_eq!(check(20, BatteryStatus::NotCharging), 0);
        assert_eq!(check(30, BatteryStatus::Charging), 0);
        assert_eq!(check(20, BatteryStatus::NotCharging), 1);
    }

    #[test]
    fn ignores_unknown_and_charging_readings() {
        let mut alerts = BatteryAlerts::default();
        for (level, status) in [(255, BatteryStatus::NotCharging),
            (0, BatteryStatus::Disconnected), (10, BatteryStatus::Charging)] {
            assert!(alerts.update("device", &[reading(BatteryComponent::Case, level, status)]).is_empty());
        }
        assert_eq!(alerts.update("device", &[reading(BatteryComponent::Case, 0, BatteryStatus::NotCharging)]), ["Charging case: 0%"]);
    }

    #[test]
    fn components_and_devices_have_independent_alerts() {
        let mut alerts = BatteryAlerts::default();
        let readings = [BatteryComponent::Left, BatteryComponent::Right,
            BatteryComponent::Case, BatteryComponent::Headphone]
            .map(|component| reading(component, 10, BatteryStatus::NotCharging));
        assert_eq!(alerts.update("first", &readings).len(), 4);
        assert!(alerts.update("first", &[]).is_empty());
        assert!(alerts.update("first", &readings).is_empty());
        assert_eq!(alerts.update("second", &readings).len(), 4);
    }
}
