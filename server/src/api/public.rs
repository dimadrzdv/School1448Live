use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use super::{ApiError, ApiResult, AppState, ClientIp, RecordingDto, StreamDto};
use crate::db;
use crate::sfu::LayerPref;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/streams", get(list_streams))
        .route("/streams/{id}", get(get_stream))
        .route("/streams/{id}/whep", post(whep))
        .route("/whep/{session}", delete(whep_delete))
        .route("/whep/{session}/layer", post(whep_layer))
        .route("/recordings", get(list_recordings))
        .route("/recordings/{id}", get(get_recording))
        .route("/recordings/{id}/view", post(add_view))
        .route("/events", get(events))
}

async fn list_streams(State(s): State<AppState>) -> ApiResult<Json<Vec<StreamDto>>> {
    let streams = db::active_streams(&s.db).await?;
    Ok(Json(streams.into_iter().map(|st| StreamDto::new(st, &s.hub, None)).collect()))
}

async fn get_stream(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<StreamDto>> {
    let stream = db::get_stream(&s.db, &id).await?.ok_or(ApiError::NotFound)?;
    // After the broadcast the page can offer the recording.
    let recording: Option<(String,)> = if stream.status == "ended" {
        sqlx::query_as("SELECT id FROM recordings WHERE stream_id = ? AND status = 'ready' AND hidden = 0")
            .bind(&id)
            .fetch_optional(&s.db)
            .await?
    } else {
        None
    };
    Ok(Json(StreamDto::new(stream, &s.hub, recording.map(|r| r.0))))
}

/// WHEP: the viewer posts an SDP offer and gets the answer back in one round trip.
async fn whep(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: String,
) -> ApiResult<Response> {
    check_sdp(&headers, &body)?;
    let answer = s.sfu.view(id, body, ip).await?;
    Ok(sdp_created(answer.sdp, format!("/api/whep/{}", answer.session)))
}

async fn whep_delete(State(s): State<AppState>, Path(session): Path<String>) -> StatusCode {
    s.sfu.close_session(session);
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct LayerBody {
    layer: LayerPref,
}

async fn whep_layer(
    State(s): State<AppState>,
    Path(session): Path<String>,
    Json(body): Json<LayerBody>,
) -> StatusCode {
    s.sfu.set_layer(session, body.layer);
    StatusCode::NO_CONTENT
}

async fn list_recordings(
    State(s): State<AppState>,
    Query(q): Query<db::RecordingQuery>,
) -> ApiResult<Json<Value>> {
    let (items, total) = db::list_recordings(&s.db, &q, false).await?;
    let years = db::recording_years(&s.db).await?;
    let items: Vec<RecordingDto> = items.into_iter().map(Into::into).collect();
    Ok(Json(json!({ "items": items, "total": total, "years": years })))
}

async fn get_recording(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<RecordingDto>> {
    let rec = db::get_recording(&s.db, &id).await?.ok_or(ApiError::NotFound)?;
    if rec.hidden || rec.status != "ready" {
        return Err(ApiError::NotFound);
    }
    Ok(Json(rec.into()))
}

/// Counts a view once per client and recording per day (in memory).
async fn add_view(State(s): State<AppState>, Path(id): Path<String>, ClientIp(ip): ClientIp) -> ApiResult<StatusCode> {
    type Seen = (i64, HashSet<(String, std::net::IpAddr)>);
    static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new((0, HashSet::new())));
    let day = crate::util::now_ms() / 86_400_000;
    let first = {
        let mut seen = SEEN.lock().unwrap();
        if seen.0 != day {
            *seen = (day, HashSet::new());
        }
        seen.1.insert((id.clone(), ip))
    };
    if first {
        db::add_view(&s.db, &id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn events(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = s.hub.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(text) => Some(Ok(Event::default().data(text.as_ref()))),
        // A lagging client just refetches everything.
        Err(_) => Some(Ok(Event::default().data(r#"{"type":"streams"}"#))),
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}

pub(super) fn check_sdp(headers: &HeaderMap, body: &str) -> ApiResult<()> {
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !ct.starts_with("application/sdp") {
        return Err(ApiError::BadRequest("Ожидается Content-Type: application/sdp".into()));
    }
    if body.len() > 64 * 1024 || !body.starts_with("v=0") {
        return Err(ApiError::BadRequest("Некорректное SDP".into()));
    }
    Ok(())
}

pub(super) fn sdp_created(sdp: String, location: String) -> Response {
    (
        StatusCode::CREATED,
        [(header::CONTENT_TYPE, "application/sdp".to_string()), (header::LOCATION, location)],
        sdp,
    )
        .into_response()
}
