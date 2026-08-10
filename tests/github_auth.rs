use std::collections::BTreeMap;

use pitools::github::auth::InstallationTokenScope;

#[test]
fn installation_scope_serializes_only_explicit_runtime_permissions() {
    let mut permissions = BTreeMap::new();
    permissions.insert("checks".into(), "write".into());
    let value = serde_json::to_value(InstallationTokenScope {
        permissions,
        repositories: vec!["PiTools".into()],
    })
    .expect("scope serializes");
    assert_eq!(value["permissions"]["checks"], "write");
    assert_eq!(value["repositories"][0], "PiTools");
    assert!(!value.to_string().contains("private"));
}

#[test]
fn empty_installation_scope_omits_optional_request_fields() {
    let scope = InstallationTokenScope::default();
    assert!(scope.is_empty());
    assert_eq!(
        serde_json::to_value(scope).expect("scope serializes"),
        serde_json::json!({})
    );
}
