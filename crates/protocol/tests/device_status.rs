use androidconnect_protocol::{
    AudioControl, AudioControlCommand, BluetoothState, DeviceStatus, DndMode, DndState, Envelope,
    FeatureStatus, NotificationFilterUpdate, PROTOCOL_VERSION, Payload, UtilityFeature,
    VolumeState, WifiState, decode_envelope, encode_envelope,
};
use serde::{Deserialize, Serialize};

fn populated_device_status() -> DeviceStatus {
    DeviceStatus {
        device_id: "phone-1".to_owned(),
        device_name: "Pixel test device".to_owned(),
        manufacturer: "Google".to_owned(),
        model: "Pixel".to_owned(),
        android_sdk: 35,
        battery_percent: Some(72),
        charging: Some(false),
        interactive: Some(true),
        features: vec![FeatureStatus::available(UtilityFeature::DeviceStatus)],
        wifi_state: Some(WifiState {
            connected: true,
            ssid: Some("home-2.4".to_owned()),
            signal_strength: Some(-58),
        }),
        bluetooth_state: Some(BluetoothState {
            enabled: true,
            connected_devices: 2,
        }),
        dnd_state: Some(DndState {
            enabled: true,
            mode: DndMode::Priority,
        }),
        volume: Some(VolumeState {
            media_percent: 50,
            ring_percent: Some(30),
            max_percent: 100,
        }),
    }
}

#[test]
fn device_status_round_trips_with_new_fields() {
    let envelope = Envelope::new(42, Payload::DeviceStatus(populated_device_status()));
    let bytes = encode_envelope(&envelope).expect("encode");
    let decoded = decode_envelope(&bytes).expect("decode");
    assert_eq!(decoded.version, PROTOCOL_VERSION);
    assert_eq!(decoded.payload, envelope.payload);
}

#[test]
fn device_status_round_trips_with_only_legacy_fields() {
    let status = DeviceStatus {
        device_id: "phone-1".to_owned(),
        device_name: "Pixel test device".to_owned(),
        manufacturer: "Google".to_owned(),
        model: "Pixel".to_owned(),
        android_sdk: 35,
        battery_percent: Some(72),
        charging: Some(false),
        interactive: Some(true),
        features: vec![FeatureStatus::available(UtilityFeature::DeviceStatus)],
        wifi_state: None,
        bluetooth_state: None,
        dnd_state: None,
        volume: None,
    };
    let envelope = Envelope::new(43, Payload::DeviceStatus(status));
    let bytes = encode_envelope(&envelope).expect("encode");
    let decoded = decode_envelope(&bytes).expect("decode");
    assert_eq!(decoded.payload, envelope.payload);
}

#[test]
fn audio_control_quick_setting_variants_round_trip() {
    for command in [
        AudioControlCommand::Start,
        AudioControlCommand::Stop,
        AudioControlCommand::Mute,
        AudioControlCommand::Unmute,
        AudioControlCommand::SetVolume { percent: 73 },
        AudioControlCommand::SetDnd {
            mode: DndMode::TotalSilence,
        },
        AudioControlCommand::SetBluetooth { enabled: true },
    ] {
        let envelope = Envelope::new(1, Payload::AudioControl(AudioControl { command }));
        let bytes = encode_envelope(&envelope).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        assert_eq!(decoded.payload, envelope.payload, "variant {command:?}");
    }
}

#[test]
fn notification_filter_update_round_trips() {
    let envelope = Envelope::new(
        99,
        Payload::NotificationFilterUpdate(NotificationFilterUpdate {
            package_name: "com.example.app".to_owned(),
            enabled: false,
        }),
    );
    let bytes = encode_envelope(&envelope).expect("encode");
    let decoded = decode_envelope(&bytes).expect("decode");
    assert_eq!(decoded.payload, envelope.payload);
}

/// Mirror of MVP 2's `DeviceStatus` (protocol v4) before the MVP 3 prework extended it. The new
/// optional fields use `#[serde(default)]` in the JSON path so an Android build still on the v4
/// schema can submit JSON without them and `push_device_status_json` still parses cleanly.
#[derive(Serialize, Deserialize)]
struct LegacyDeviceStatusJson {
    device_name: String,
    manufacturer: String,
    model: String,
    android_sdk: u32,
    battery_percent: Option<u8>,
    charging: Option<bool>,
    interactive: Option<bool>,
    features: Vec<FeatureStatus>,
}

/// The new fields on the live `DeviceStatus` use `#[serde(default)]`, so JSON shaped like the
/// legacy v4 schema must still deserialize into the v5 type. This protects the JNI JSON path that
/// Android uses to push status updates (see `android-native::push_device_status_json`).
#[derive(Deserialize)]
struct CurrentDeviceStatusJson {
    #[serde(default)]
    wifi_state: Option<WifiState>,
    #[serde(default)]
    bluetooth_state: Option<BluetoothState>,
    #[serde(default)]
    dnd_state: Option<DndState>,
    #[serde(default)]
    volume: Option<VolumeState>,
}

#[test]
fn legacy_json_payload_still_parses_into_current_schema() {
    let legacy = LegacyDeviceStatusJson {
        device_name: "Pixel".to_owned(),
        manufacturer: "Google".to_owned(),
        model: "Pixel".to_owned(),
        android_sdk: 35,
        battery_percent: Some(80),
        charging: None,
        interactive: Some(true),
        features: vec![FeatureStatus::available(UtilityFeature::DeviceStatus)],
    };
    let json = serde_json::to_string(&legacy).expect("encode legacy");
    let parsed: CurrentDeviceStatusJson = serde_json::from_str(&json).expect("decode current");
    assert!(parsed.wifi_state.is_none());
    assert!(parsed.bluetooth_state.is_none());
    assert!(parsed.dnd_state.is_none());
    assert!(parsed.volume.is_none());
}
