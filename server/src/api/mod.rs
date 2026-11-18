mod admin;
mod public;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::LoginLimiter;
use crate::config::Config;
use crate::db::{self, Db};
use crate::hub::Hub;
use crate::sfu::{SfuError, SfuHandle};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub sfu: SfuHandle,
    pub hub: Arc<Hub>,
    pub limiter: Arc<LoginLimiter>,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(public::routes())
        .merge(admin::routes())
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(CompressionLayer::new());

    let media = Router::new()
        .fallback_service(ServeDir::new(state.config.public_dir()))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=3600"),
        ));

    let mut app = Router::new().nest("/api", api).nest("/media", media);

    // Production normally serves the web app from Caddy; this keeps a single binary usable too.
    // Media segments are already compressed, so only the API and the app get gzip/br.
    if let Some(web) = &state.config.web_dir {
        let index = web.join("index.html");
        let spa = Router::new()
            .fallback_service(ServeDir::new(web).fallback(ServeFile::new(index)))
            .layer(CompressionLayer::new());
        app = app.merge(spa);
    }

    app.with_state(state)
}

/// Client IP: the socket peer, or `X-Forwarded-For` from the trusted reverse proxy.
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if state.config.trust_proxy {
            let forwarded = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse().ok());
            if let Some(ip) = forwarded {
                return Ok(ClientIp(ip));
            }
        }
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        Ok(ClientIp(peer))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Требуется вход для администрации")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("Не найдено")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("Слишком много попыток. Попробуйте через {0} с")]
    TooManyAttempts(u64),
    #[error("{0}")]
    Sfu(#[from] SfuError),
    #[error("Внутренняя ошибка сервера")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::TooManyAttempts(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Sfu(SfuError::NotLive) => StatusCode::NOT_FOUND,
            ApiError::Sfu(SfuError::Busy) => StatusCode::CONFLICT,
            ApiError::Sfu(SfuError::TooManyViewers) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Sfu(SfuError::BadOffer(_)) => StatusCode::BAD_REQUEST,
            ApiError::Sfu(SfuError::Unavailable) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let code = match &self {
            ApiError::Sfu(SfuError::Busy) => "busy",
            ApiError::Sfu(SfuError::NotLive) => "not_live",
            _ => "error",
        };
        (status, axum::Json(json!({ "error": self.to_string(), "code": code }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Response shapes

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    /// scheduled | live | reconnecting | ended
    pub status: &'static str,
    pub scheduled_at: Option<i64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub viewers: u32,
    pub poster_url: Option<String>,
    pub recording_id: Option<String>,
}

impl StreamDto {
    pub fn new(s: db::Stream, hub: &Hub, recording_id: Option<String>) -> Self {
        let rt = hub.runtime(&s.id);
        let status = match s.status.as_str() {
            "live" => match rt {
                Some(r) if r.publisher => "live",
                _ => "reconnecting",
            },
            "scheduled" => "scheduled",
            _ => "ended",
        };
        StreamDto {
            poster_url: s.poster_at.map(|v| format!("/media/posters/{}.jpg?v={v}", s.id)),
            viewers: rt.map(|r| r.viewers).unwrap_or(0),
            id: s.id,
            title: s.title,
            description: s.description,
            category: s.category,
            status,
            scheduled_at: s.scheduled_at,
            started_at: s.started_at,
            ended_at: s.ended_at,
            recording_id,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingDto {
    pub id: String,
    pub stream_id: Option<String>,
    pub title: String,
    pub description: String,
    pub category: String,
    pub status: String,
    pub hidden: bool,
    pub recorded_at: i64,
    pub duration_ms: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub thumbnail_url: Option<String>,
    pub playlist_url: Option<String>,
    pub qualities: Vec<u32>,
    pub views: i64,
    pub error: Option<String>,
}

impl From<db::Recording> for RecordingDto {
    fn from(r: db::Recording) -> Self {
        let renditions: Vec<db::Rendition> = serde_json::from_str(&r.renditions).unwrap_or_default();
        let ready = r.status == "ready";
        let mut qualities: Vec<u32> = renditions.iter().map(|x| x.height).collect();
        qualities.sort_unstable_by(|a, b| b.cmp(a));
        RecordingDto {
            thumbnail_url: r.has_thumb.then(|| format!("/media/rec/{}/thumb.jpg", r.id)),
            playlist_url: ready.then(|| format!("/media/rec/{}/master.m3u8", r.id)),
            qualities,
            id: r.id,
            stream_id: r.stream_id,
            title: r.title,
            description: r.description,
            category: r.category,
            status: r.status,
            hidden: r.hidden,
            recorded_at: r.recorded_at,
            duration_ms: r.duration_ms,
            width: r.width,
            height: r.height,
            views: r.views,
            error: r.error,
        }
    }
}
