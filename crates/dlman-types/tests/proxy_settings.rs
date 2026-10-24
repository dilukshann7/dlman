use dlman_types::{ProxySettings, Settings};
use serde_json::json;

#[test]
fn default_settings_use_the_system_proxy_mode() {
    assert_eq!(ProxySettings::default().mode, "system");
    assert_eq!(Settings::default().proxy.mode, "system");
}

#[test]
fn legacy_empty_proxy_mode_is_repaired_when_settings_are_loaded() {
    let settings: ProxySettings = serde_json::from_value(json!({
        "mode": "",
        "http_proxy": "http://proxy.example:8080",
        "https_proxy": null,
        "no_proxy": "localhost",
        "username": "user",
        "password": "password"
    }))
    .unwrap();
    assert_eq!(settings.mode, "system");
    assert_eq!(
        settings.http_proxy.as_deref(),
        Some("http://proxy.example:8080")
    );
    assert_eq!(settings.no_proxy.as_deref(), Some("localhost"));
    assert_eq!(settings.username.as_deref(), Some("user"));
    assert_eq!(settings.password.as_deref(), Some("password"));
    assert_eq!(serde_json::to_value(settings).unwrap()["mode"], "system");
}

#[test]
fn absent_proxy_fields_use_the_same_defaults_as_new_settings() {
    let settings: ProxySettings = serde_json::from_value(json!({})).unwrap();
    assert_eq!(settings.mode, "system");
    assert!(settings.http_proxy.is_none());
    assert!(settings.https_proxy.is_none());
    assert!(settings.no_proxy.is_none());
    assert!(settings.username.is_none());
    assert!(settings.password.is_none());
}

#[test]
fn explicit_proxy_modes_are_preserved() {
    for mode in ["none", "system", "manual", "unsupported"] {
        let settings: ProxySettings = serde_json::from_value(json!({ "mode": mode })).unwrap();
        assert_eq!(settings.mode, mode);
    }
}
