use argon2::{Argon2, PasswordHash, PasswordVerifier};
use secrecy::ExposeSecret;

pub fn verify_bearer_token(token: &str, stored_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(token.as_bytes(), &hash)
        .is_ok()
}

pub fn validate_bearer_hash(value: &str) -> Result<(), String> {
    PasswordHash::new(value)
        .map(|_| ())
        .map_err(|error| format!("invalid Argon2 password hash: {error}"))
}

pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
}

pub fn is_authorized(headers: &axum::http::HeaderMap, stored_hash: &secrecy::SecretString) -> bool {
    bearer_token(headers)
        .map(|token| verify_bearer_token(token, stored_hash.expose_secret()))
        .unwrap_or(false)
}
