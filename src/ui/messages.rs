use crate::bluetooth::aacp::AACPEvent;

#[derive(Debug, Clone)]
pub enum BluetoothUIMessage {
    DevicesChanged(std::collections::HashMap<String, crate::devices::enums::DeviceData>),
    PairingError(String),
    PairingReady,
    NearbyAirPods(crate::bluetooth::proximity::NearbyAirPods),
    NearbyLost(String),
    DiscoveryError(String),
    DiscoveryReady,
    OpenWindow,
    TrayReady(super::tray::TrayHandle),
    DeviceConnected(String),        // mac
    DeviceDisconnected(String),     // mac
    AACPUIEvent(String, AACPEvent), // mac, event
}
