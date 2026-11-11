# Desktop integration

The running application exposes a session D-Bus service, including in headless
mode (`airpods-driver --no-tray`). No root access is required. This API is a
foundation for desktop extensions and keyboard shortcuts; a GNOME extension or
Plasma widget is not included.

- Bus name: `com.vitalyart.AirpodsDriver`
- Object: `/com/vitalyart/AirpodsDriver`
- Interface: `com.vitalyart.AirpodsDriver1`

`GetDevices() → s` returns a JSON object keyed by Bluetooth address, containing
initialized devices. Each entry contains `connected`, `listening_mode` (a byte or
null when unknown), and `batteries`. Battery entries contain `component`,
`percentage`, and `charging`. Component numbers are 1 (whole headset), 2 (right),
4 (left), and 8 (case). Unknown and disconnected battery readings are omitted.
Only these public fields are exported; pairing and encryption keys are excluded.

`SetListeningMode(s address, y mode)` sends a command to a connected device.
Modes are 1 (off), 2 (noise cancellation), 3 (transparency), and 4 (adaptive).
Device and firmware support still apply. Successful return means the command
was sent, not that the device has confirmed the change. Read the reported state
or subscribe to `DevicesChanged(s devices_json)`, which carries a complete
snapshot when state changes. State is sampled approximately once per second.
Invalid arguments, unknown devices and transport errors return D-Bus errors.

Examples:

```sh
busctl --user call com.vitalyart.AirpodsDriver /com/vitalyart/AirpodsDriver \
  com.vitalyart.AirpodsDriver1 GetDevices
busctl --user call com.vitalyart.AirpodsDriver /com/vitalyart/AirpodsDriver \
  com.vitalyart.AirpodsDriver1 SetListeningMode sy AA:BB:CC:DD:EE:FF 2
```

## System battery indicator

On the system bus the application registers a BlueZ `BatteryProvider1` using
`BatteryProviderManager1` for each available adapter. It publishes the lowest
valid earbud/headset percentage as the device's aggregate charge. Case charge
is excluded from this aggregate and remains available through the session API.
Provider objects are removed when BlueZ reports the device disconnected or
there is no valid earbud reading. Nearby LE advertisements are not published.
The provider supports `ObjectManager.GetManagedObjects`, `InterfacesAdded`,
`InterfacesRemoved`, and `PropertiesChanged`.

This requires BlueZ to expose BatteryProviderManager1 and system bus policy to
permit provider registration. Availability varies with the installed BlueZ
build and configuration. The application does not enable experimental BlueZ
features, change system policy, or override another battery source. The desktop
decides how to display BlueZ battery information. On Ubuntu, check Settings →
Power for the headphone battery; the Bluetooth menu may omit the percentage. Registration failures are
logged and retried approximately every 30 seconds, including after a BlueZ
restart. Other application features continue when registration is unavailable.

Flatpak packaging permits ownership of the session API name and already grants
access to BlueZ; host system policy still applies.

## Validation

```sh
cargo check --offline
cargo test --offline -- --test-threads=1
# The ignored wire-protocol test must use its own bus, never a desktop session:
dbus-run-session -- cargo test --offline system::integration -- --include-ignored --test-threads=1
```

For hardware verification, connect AirPods, inspect the API with `busctl`, change
a listening mode, and inspect `org.bluez.Battery1` on the device using
`busctl --system introspect org.bluez /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`.
Disconnect the headphones and verify the provider object disappears. Test with
more than one adapter and after restarting BlueZ on a test machine.
