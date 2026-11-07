//! Apple proximity advertisements. Layout and model IDs are derived from upstream LibrePods' C++ BLE manager.
use super::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};

#[derive(Debug, Clone)]
pub struct NearbyAirPods {
    pub id: String,
    pub name: String,
    /// A resolved, paired address or a discovered Bluetooth audio device, never an unresolved RPA.
    pub connect_address: Option<String>,
    pub paired: bool,
    pub connected: bool,
    pub battery: Vec<BatteryInfo>,
    pub lid_open: Option<bool>,
    pub lid_counter: Option<u8>,
    pub pairing: bool,
}

pub struct Advertisement {
    pub model: &'static str,
    pub battery: Vec<BatteryInfo>,
    pub lid_open: Option<bool>,
    pub lid_counter: Option<u8>,
    pub pairing: bool,
    pub pairing_address: Option<bluer::Address>,
}

pub fn parse(data: &[u8]) -> Option<Advertisement> {
    // Company ID 0x004c is stripped by BlueZ. Reject unrelated Apple messages,
    // unknown products (including Beats) and incomplete length-prefixed payloads.
    if data.len() < 5 || data[0] != 0x07 || usize::from(data[1]) + 2 > data.len() {
        return None;
    }
    let model = match u16::from_be_bytes([data[3], data[4]]) {
        0x0220 => "AirPods",
        0x0f20 => "AirPods 2",
        0x1320 => "AirPods 3",
        0x1920 => "AirPods 4",
        0x1b20 => "AirPods 4 (ANC)",
        0x0a20 | 0x1f20 => "AirPods Max",
        0x0e20 => "AirPods Pro",
        0x1420 | 0x2420 => "AirPods Pro 2",
        _ => return None,
    };
    if data[2] == 0 {
        // Legacy pairing messages include the Classic address in network byte order.
        // See https://redballoonsecurity.com/baets/ (Proximity Pairing format).
        if data.len() < 17 || data[1] < 15 {
            return None;
        }
        let bytes: [u8; 6] = data[5..11].try_into().ok()?;
        if bytes == [0; 6] || bytes == [0xff; 6] {
            return None;
        }
        return Some(Advertisement {
            model,
            battery: vec![],
            lid_open: None,
            lid_counter: None,
            pairing: true,
            pairing_address: Some(bluer::Address(bytes)),
        });
    }
    if data[2] != 1 || data.len() < 27 || data[1] < 25 {
        return None;
    }
    let left_primary = data[5] & 0x20 != 0;
    let primary = data[6] & 0x0f;
    let secondary = data[6] >> 4;
    let flags = data[7] >> 4;
    let mut battery = vec![];
    for (component, nibble, charging) in [
        (
            BatteryComponent::Left,
            if left_primary { primary } else { secondary },
            flags & if left_primary { 1 } else { 2 } != 0,
        ),
        (
            BatteryComponent::Right,
            if left_primary { secondary } else { primary },
            flags & if left_primary { 2 } else { 1 } != 0,
        ),
        (BatteryComponent::Case, data[7] & 0x0f, flags & 4 != 0),
    ] {
        if nibble <= 10 {
            battery.push(BatteryInfo {
                component,
                level: nibble * 10,
                status: if charging {
                    BatteryStatus::Charging
                } else {
                    BatteryStatus::NotCharging
                },
            });
        }
    }
    Some(Advertisement {
        model,
        battery,
        lid_open: (data[5] & 0x40 != 0).then_some(data[8] & 8 == 0),
        lid_counter: Some(data[8] & 7),
        pairing: false,
        pairing_address: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn packet() -> Vec<u8> {
        let mut data = vec![0; 27];
        data[..11].copy_from_slice(&[7, 25, 1, 0x14, 0x20, 0x60, 0x78, 0x46, 2, 0, 0]);
        data
    }
    #[test]
    fn rejects_truncation_other_messages_and_other_products() {
        let data = packet();
        for end in 0..data.len() {
            assert!(parse(&data[..end]).is_none());
        }
        for (index, value) in [(0, 0x12), (1, 26), (2, 2), (3, 0xff)] {
            let mut invalid = data.clone();
            invalid[index] = value;
            assert!(parse(&invalid).is_none());
        }
    }
    #[test]
    fn decodes_lid_battery_orientation_and_unknown_values() {
        let mut data = packet();
        let ad = parse(&data).unwrap();
        assert_eq!(ad.lid_open, Some(true));
        assert_eq!(ad.lid_counter, Some(2));
        assert_eq!(
            ad.battery.iter().map(|b| b.level).collect::<Vec<_>>(),
            [80, 70, 60]
        );
        assert_eq!(ad.battery[2].status, BatteryStatus::Charging);
        data[5] = 0x40;
        data[8] = 8;
        let ad = parse(&data).unwrap();
        assert_eq!(ad.battery[0].level, 70);
        assert_eq!(ad.lid_open, Some(false));
        data[6] = 0xff;
        data[7] = 0x0b;
        assert!(parse(&data).unwrap().battery.is_empty());
        data[5] = 0;
        assert_eq!(parse(&data).unwrap().lid_open, None);
    }
    #[test]
    fn pairing_payload_does_not_expose_battery_or_lid() {
        let mut data = vec![
            7, 15, 0, 0x14, 0x20, 0x41, 0x42, 9, 0xd4, 0x31, 0x51, 0x95, 100, 100, 100, 2, 0,
        ];
        for end in 0..data.len() {
            assert!(parse(&data[..end]).is_none());
        }
        let ad = parse(&data).unwrap();
        assert_eq!(ad.pairing_address.unwrap().to_string(), "41:42:09:D4:31:51");
        data[5..11].fill(0);
        assert!(parse(&data).is_none());
        assert!(ad.pairing);
        assert!(ad.battery.is_empty());
        assert_eq!(ad.lid_open, None);
    }
}
