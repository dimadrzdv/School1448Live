use std::env;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Runtime configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP listen address for the API (Caddy proxies to it).
    pub http_addr: SocketAddr,
    /// Root for the database, raw recordings and public media files.
    pub data_dir: PathBuf,
    /// Optional directory with the built web app; served with SPA fallback.
    pub web_dir: Option<PathBuf>,
    /// Argon2id PHC hash of the admin secret code.
    pub admin_code_hash: String,
    /// Allowed browser origins for admin requests (exact match). Empty = no check.
    pub allowed_origins: Vec<String>,
    /// Whether the session cookie gets the `Secure` flag.
    pub cookie_secure: bool,
    /// Trust `X-Forwarded-For` from the reverse proxy.
    pub trust_proxy: bool,
    pub media: MediaConfig,
}

#[derive(Debug, Clone)]
pub struct MediaConfig {
    /// UDP and TCP port for all WebRTC traffic.
    pub port: u16,
    /// Address of the local interface that receives media (host candidate).
    pub lan_ip: IpAddr,
    /// Public address reachable from the internet (advertised as srflx candidate).
    pub public_ip: Option<IpAddr>,
    /// Upstream budget for viewers outside the local network, in bits per second.
    pub egress_limit_bps: u64,
    /// Seconds a live stream waits for its publisher to come back.
    pub reconnect_grace_secs: u64,
    /// Maximum concurrent viewer sessions per public client IP; 0 = unlimited.
    /// Local-network addresses are not limited.
    pub max_viewers_per_ip: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let http_addr = var_or("HTTP_ADDR", "0.0.0.0:8080")
            .parse()
            .context("HTTP_ADDR")?;
        // Absolute, so paths handed to ffmpeg (concat lists) resolve correctly.
        let data_dir = std::path::absolute(PathBuf::from(var_or("DATA_DIR", "./data")))
            .context("DATA_DIR")?;
        let web_dir = env::var("WEB_DIR").ok().filter(|s| !s.is_empty()).map(PathBuf::from);

        let admin_code_hash = match (env::var("ADMIN_CODE_HASH"), env::var("ADMIN_CODE")) {
            (Ok(h), _) if !h.trim().is_empty() => {
                // Tolerate quotes copied from .env along with the value.
                let h = h.trim().trim_matches('\'').trim_matches('"').to_string();
                if !crate::auth::is_valid_hash(&h) {
                    bail!("ADMIN_CODE_HASH повреждён: получите новый командой `schoollive hash-code`");
                }
                h
            }
            (_, Ok(code)) if !code.is_empty() => {
                tracing::warn!(
                    "ADMIN_CODE задан открытым текстом. Для продакшна используйте ADMIN_CODE_HASH \
                     (команда `schoollive hash-code`)."
                );
                crate::auth::hash_code(&code)?
            }
            _ => bail!("Не задан ADMIN_CODE_HASH (или ADMIN_CODE для разработки)"),
        };

        let allowed_origins = env::var("ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();

        let cookie_secure = bool_var("COOKIE_SECURE", true);
        let trust_proxy = bool_var("TRUST_PROXY", false);

        let lan_ip = match env::var("MEDIA_LAN_IP").ok().filter(|s| !s.is_empty()) {
            Some(s) => s.parse().context("MEDIA_LAN_IP")?,
            None => detect_lan_ip().context(
                "Не удалось определить IP локальной сети, задайте MEDIA_LAN_IP",
            )?,
        };
        let public_ip = match env::var("MEDIA_PUBLIC_IP").ok().filter(|s| !s.is_empty()) {
            Some(s) => Some(s.parse().context("MEDIA_PUBLIC_IP")?),
            None => None,
        };

        let media = MediaConfig {
            port: var_or("MEDIA_PORT", "8189").parse().context("MEDIA_PORT")?,
            lan_ip,
            public_ip: public_ip.filter(|ip| *ip != lan_ip),
            egress_limit_bps: var_or("EGRESS_LIMIT_MBPS", "200")
                .parse::<f64>()
                .context("EGRESS_LIMIT_MBPS")
                .map(|m| (m * 1_000_000.0) as u64)?,
            reconnect_grace_secs: var_or("RECONNECT_GRACE_SECS", "60")
                .parse()
                .context("RECONNECT_GRACE_SECS")?,
            max_viewers_per_ip: var_or("MAX_VIEWERS_PER_IP", "20")
                .parse()
                .context("MAX_VIEWERS_PER_IP")?,
        };

        Ok(Config {
            http_addr,
            data_dir,
            web_dir,
            admin_code_hash,
            allowed_origins,
            cookie_secure,
            trust_proxy,
            media,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("schoollive.sqlite")
    }
    /// Raw recordings (never served publicly).
    pub fn raw_dir(&self) -> PathBuf {
        self.data_dir.join("raw")
    }
    /// Files served under `/media`.
    pub fn public_dir(&self) -> PathBuf {
        self.data_dir.join("public")
    }
}

fn var_or(name: &str, default: &str) -> String {
    env::var(name).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

fn bool_var(name: &str, default: bool) -> bool {
    match env::var(name).ok().as_deref().map(str::trim) {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

/// Finds the address of the interface used for outbound traffic (no packets are sent).
fn detect_lan_ip() -> Result<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}
