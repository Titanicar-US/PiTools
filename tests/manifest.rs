use pitools::github::manifest::{
    AppManifest, ManifestConversion, ManifestError, validate_manifest_code,
};

#[test]
fn public_manifest_contains_the_full_watch_and_repair_contract() {
    let manifest = AppManifest::for_public_project("https://pitools.example.test");
    manifest.validate().expect("valid production manifest");
    assert_eq!(
        manifest.hook_attributes.url,
        "https://pitools.example.test/github/webhook"
    );
    assert_eq!(manifest.default_permissions["checks"], "write");
    assert_eq!(manifest.default_permissions["contents"], "write");
    assert!(
        manifest
            .default_events
            .iter()
            .any(|event| event == "workflow_run")
    );
    assert!(
        manifest
            .default_events
            .iter()
            .any(|event| event == "pull_request_review_comment")
    );
}

#[test]
fn manifest_rejects_non_https_and_wrong_webhook_paths() {
    let mut manifest = AppManifest::for_public_project("http://pitools.example.test");
    assert!(matches!(
        manifest.validate(),
        Err(ManifestError::InvalidEndpoint { .. })
    ));

    manifest = AppManifest::for_public_project("https://pitools.example.test");
    manifest.hook_attributes.url = "https://pitools.example.test/hooks/github".into();
    assert_eq!(manifest.validate(), Err(ManifestError::InvalidWebhookPath));
}

#[test]
fn manifest_conversion_codes_are_bounded_and_path_safe() {
    validate_manifest_code("a180b1a3d263c81bc6441d7b990bae27d4c10679")
        .expect("GitHub conversion code");
    assert_eq!(
        validate_manifest_code("../conversion"),
        Err(ManifestError::InvalidCode)
    );
    assert_eq!(validate_manifest_code(""), Err(ManifestError::InvalidCode));
}

#[test]
fn manifest_conversion_response_serializes_the_operator_secret_contract() {
    let response = ManifestConversion {
        id: 42,
        name: "PiTools".into(),
        slug: "pitools".into(),
        client_id: "client-id".into(),
        client_secret: "client-secret".into(),
        webhook_secret: "webhook-secret".into(),
        pem: "private-key".into(),
        html_url: Some("https://github.com/apps/pitools".into()),
    };
    let value = serde_json::to_value(response).expect("serialize conversion");
    assert_eq!(value["id"], 42);
    assert_eq!(value["webhook_secret"], "webhook-secret");
    assert_eq!(value["pem"], "private-key");
}
