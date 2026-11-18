//! Admin access by secret code: Argon2id hash, rate-limited login, server-side sessions.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::header::{COOKIE, ORIGIN};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use sha2::{Digest, Sha256};

use crate::api::{ApiError, AppState};
use crate::db;
use crate::util::{now_ms, secret_token};

pub const COOKIE_NAME: &str = "sl_admin";
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

const WINDOW: Duration = Duration::from_secs(15 * 60);
const MAX_PER_IP: usize = 5;
const MAX_GLOBAL: usize = 50;

pub fn hash_code(code: &str) -> Result<String> {
    Argon2::default()
        .hash_password(code.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("argon2: {e}"))
}

pub fn is_valid_hash(hash: &str) -> bool {
    PasswordHash::new(hash).is_ok()
}

pub fn verify_code(hash: &str, code: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default().verify_password(code.as_bytes(), &parsed).is_ok()
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Failed login attempts per IP and in total, in a sliding window.
#[derive(Default)]
pub struct LoginLimiter {
    inner: Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    per_ip: HashMap<IpAddr, Vec<Instant>>,
    global: Vec<Instant>,
}

impl LoginLimiter {
    /// Returns seconds to wait if the client is currently blocked.
    pub fn check(&self, ip: IpAddr) -> Option<u64> {
        let mut s = self.inner.lock().unwrap();
        let now = Instant::now();
        s.global.retain(|t| now.duration_since(*t) < WINDOW);
        s.per_ip.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < WINDOW);
            !v.is_empty()
        });
        let ip_list = s.per_ip.get(&ip).map(Vec::as_slice).unwrap_or(&[]);
        let blocked_by = if ip_list.len() >= MAX_PER_IP {
            ip_list.first().copied()
        } else if s.global.len() >= MAX_GLOBAL {
            s.global.first().copied()
        } else {
            None
        };
        blocked_by.map(|first| (WINDOW.saturating_sub(now.duration_since(first))).as_secs().max(1))
    }

    pub fn fail(&self, ip: IpAddr) {
        let mut s = self.inner.lock().unwrap();
        let now = Instant::now();
        s.per_ip.entry(ip).or_default().push(now);
        s.global.push(now);
    }

    pub fn success(&self, ip: IpAddr) {
        self.inner.lock().unwrap().per_ip.remove(&ip);
    }
}

/// Creates a session and returns the `Set-Cookie` value.
pub async fn create_session(db: &db::Db, secure: bool) -> Result<String> {
    let token = secret_token();
    db::insert_session(db, &token_hash(&token), now_ms() + SESSION_TTL.as_millis() as i64).await?;
    Ok(session_cookie(&token, SESSION_TTL.as_secs(), secure))
}

pub fn session_cookie(value: &str, max_age: u64, secure: bool) -> String {
    format!(
        "{COOKIE_NAME}={value}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE_NAME)
        .map(|(_, v)| v.to_string())
}

/// Extractor that only succeeds for requests with a valid admin session.
/// Mutating requests must also come from an allowed origin.
pub struct Admin {
    pub token_hash: String,
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if parts.method != Method::GET && parts.method != Method::HEAD {
            check_origin(&parts.headers, &state.config.allowed_origins)?;
        }
        let token = cookie_token(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let hash = token_hash(&token);
        if db::session_valid(&state.db, &hash).await? {
            Ok(Admin { token_hash: hash })
        } else {
            Err(ApiError::Unauthorized)
        }
    }
}

pub fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), ApiError> {
    if allowed.is_empty() {
        return Ok(());
    }
    match headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) if allowed.iter().any(|a| a == origin) => Ok(()),
        // Same-origin fetches from older Safari may omit Origin on non-CORS requests;
        // SameSite=Strict on the cookie still blocks cross-site use.
        None => Ok(()),
        Some(_) => Err(ApiError::Forbidden("Запрос с чужого сайта")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_code("школа-1448").unwrap();
        assert!(verify_code(&h, "школа-1448"));
        assert!(!verify_code(&h, "school-1448"));
        assert!(!verify_code("garbage", "x"));
    }

    #[test]
    fn limiter_blocks_after_five_failures() {
        let l = LoginLimiter::default();
        let ip: IpAddr = "10.0.0.7".parse().unwrap();
        for _ in 0..MAX_PER_IP {
            assert!(l.check(ip).is_none());
            l.fail(ip);
        }
        assert!(l.check(ip).is_some());
        let other: IpAddr = "10.0.0.8".parse().unwrap();
        assert!(l.check(other).is_none());
    }

    #[test]
    fn reads_cookie() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, "a=1; sl_admin=abc; b=2".parse().unwrap());
        assert_eq!(cookie_token(&h).as_deref(), Some("abc"));
    }

    #[test]
    fn origin_check() {
        let allowed = vec!["https://live.example".to_string()];
        let mut h = HeaderMap::new();
        assert!(check_origin(&h, &allowed).is_ok());
        h.insert(ORIGIN, "https://evil.example".parse().unwrap());
        assert!(check_origin(&h, &allowed).is_err());
        h.insert(ORIGIN, "https://live.example".parse().unwrap());
        assert!(check_origin(&h, &allowed).is_ok());
    }
}
