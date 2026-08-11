use std::{env, fs, net::IpAddr, path::PathBuf};

use secrecy::{ExposeSecret, SecretString};

use crate::error::ConfigError;

#[derive(Clone)]
pub struct AppConfig {
    pub bind_address: String,
    pub database_url: SecretString,
    pub nats_url: String,
    pub github_app_id: u64,
    pub github_private_key: SecretString,
    pub github_webhook_secret: SecretString,
    pub admin_bearer_token_hash: SecretString,
    pub reconcile_interval_seconds: u64,
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("bind_address", &self.bind_address)
            .field("database_url", &"[REDACTED]")
            .field("nats_url", &self.nats_url)
            .field("github_app_id", &self.github_app_id)
            .field("github_private_key", &"[REDACTED]")
            .field("github_webhook_secret", &"[REDACTED]")
            .field("admin_bearer_token_hash", &"[REDACTED]")
            .field(
                "reconcile_interval_seconds",
                &self.reconcile_interval_seconds,
            )
            .finish()
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();

        let bind_address =
            env::var("PITOOLS_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".into());
        validate_bind_address(&bind_address)?;

        let database_url = required("DATABASE_URL")?;
        let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let github_app_id =
            required("GITHUB_APP_ID")?
                .parse::<u64>()
                .map_err(|error| ConfigError::Invalid {
                    field: "GITHUB_APP_ID",
                    reason: error.to_string(),
                })?;
        let github_private_key = read_secret_path("GITHUB_PRIVATE_KEY_PATH")?;
        let github_webhook_secret = SecretString::from(required("GITHUB_WEBHOOK_SECRET")?);
        let admin_bearer_token_hash_value = required("ADMIN_BEARER_TOKEN_HASH")?;
        crate::admin::validate_bearer_hash(&admin_bearer_token_hash_value).map_err(|reason| {
            ConfigError::Invalid {
                field: "ADMIN_BEARER_TOKEN_HASH",
                reason,
            }
        })?;
        let admin_bearer_token_hash = SecretString::from(admin_bearer_token_hash_value);
        let reconcile_interval_seconds = env::var("PITOOLS_RECONCILE_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "300".into())
            .parse::<u64>()
            .map_err(|error| ConfigError::Invalid {
                field: "PITOOLS_RECONCILE_INTERVAL_SECONDS",
                reason: error.to_string(),
            })?;
        if !(30..=86_400).contains(&reconcile_interval_seconds) {
            return Err(ConfigError::Invalid {
                field: "PITOOLS_RECONCILE_INTERVAL_SECONDS",
                reason: "must be between 30 and 86400 seconds".into(),
            });
        }

        Ok(Self {
            bind_address,
            database_url: SecretString::from(database_url),
            nats_url,
            github_app_id,
            github_private_key,
            github_webhook_secret,
            admin_bearer_token_hash,
            reconcile_interval_seconds,
        })
    }

    pub fn private_key_pem(&self) -> &str {
        self.github_private_key.expose_secret()
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    env::var(name).map_err(|_| ConfigError::Missing(name))
}

fn read_secret_path(name: &'static str) -> Result<SecretString, ConfigError> {
    let path = PathBuf::from(required(name)?);
    let bytes = fs::read(&path).map_err(|error| ConfigError::Invalid {
        field: name,
        reason: format!("cannot read {}: {error}", path.display()),
    })?;
    let value = String::from_utf8(bytes).map_err(|error| ConfigError::Invalid {
        field: name,
        reason: format!("private key is not UTF-8: {error}"),
    })?;
    Ok(SecretString::from(value))
}

fn validate_bind_address(value: &str) -> Result<(), ConfigError> {
    let (host, port) = value.split_once(':').ok_or_else(|| ConfigError::Invalid {
        field: "PITOOLS_BIND_ADDRESS",
        reason: "expected host:port".into(),
    })?;
    host.parse::<IpAddr>()
        .map_err(|error| ConfigError::Invalid {
            field: "PITOOLS_BIND_ADDRESS",
            reason: error.to_string(),
        })?;
    port.parse::<u16>().map_err(|error| ConfigError::Invalid {
        field: "PITOOLS_BIND_ADDRESS",
        reason: error.to_string(),
    })?;
    Ok(())
}
