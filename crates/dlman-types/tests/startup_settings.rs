use dlman_types::Settings;

#[test]
fn new_profiles_enable_startup_by_default() {
    assert!(Settings::default().start_on_boot);
}

#[test]
fn older_payloads_without_a_startup_preference_use_the_enabled_default() {
    let mut payload = serde_json::to_value(Settings::default()).unwrap();
    payload.as_object_mut().unwrap().remove("start_on_boot");
    let settings: Settings = serde_json::from_value(payload).unwrap();
    assert!(settings.start_on_boot);
}

#[test]
fn an_explicit_opt_out_survives_a_settings_round_trip() {
    let settings = Settings {
        start_on_boot: false,
        ..Default::default()
    };
    let saved = serde_json::to_string(&settings).unwrap();
    let loaded: Settings = serde_json::from_str(&saved).unwrap();
    assert!(!loaded.start_on_boot);
}
