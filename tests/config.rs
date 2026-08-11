use std::{
    env, fs,
    sync::{Mutex, OnceLock},
};

use argon2::{Argon2, PasswordHasher};
use pitools::AppConfig;
use tempfile::NamedTempFile;

fn clear_config() {
    for key in [
        "PITOOLS_BIND_ADDRESS",
        "DATABASE_URL",
        "NATS_URL",
        "GITHUB_APP_ID",
        "GITHUB_PRIVATE_KEY_PATH",
        "GITHUB_WEBHOOK_SECRET",
        "ADMIN_BEARER_TOKEN_HASH",
        "PITOOLS_RECONCILE_INTERVAL_SECONDS",
    ] {
        // SAFETY: tests run without other threads reading these process-local variables.
        unsafe { env::remove_var(key) };
    }
}

fn environment_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("configuration test mutex")
}

#[test]
fn configuration_requires_secrets() {
    let _guard = environment_guard();
    clear_config();

    let error = AppConfig::from_env().expect_err("configuration should fail closed");

    assert!(error.to_string().contains("DATABASE_URL"));
}

#[test]
fn configuration_redacts_secret_values_in_debug_output() {
    let _guard = environment_guard();
    clear_config();
    let key = NamedTempFile::new().expect("temporary key file");
    fs::write(key.path(), "private-key-value").expect("write key");
    let admin_hash = Argon2::default()
        .hash_password(b"admin-token")
        .expect("password hash")
        .to_string();
    for (name, value) in [
        ("DATABASE_URL", "postgres://localhost/pitools"),
        ("NATS_URL", "nats://localhost:4222"),
        ("GITHUB_APP_ID", "42"),
        (
            "GITHUB_PRIVATE_KEY_PATH",
            key.path().to_str().expect("key path"),
        ),
        ("GITHUB_WEBHOOK_SECRET", "webhook-secret-value"),
        ("ADMIN_BEARER_TOKEN_HASH", admin_hash.as_str()),
    ] {
        // SAFETY: this test owns the process-local environment variables.
        unsafe { env::set_var(name, value) };
    }

    let config = AppConfig::from_env().expect("valid configuration");
    let debug = format!("{config:?}");

    assert!(!debug.contains("private-key-value"));
    assert!(!debug.contains("webhook-secret-value"));
    assert!(!debug.contains("admin-hash-value"));
    assert!(debug.contains("[REDACTED]"));
}
