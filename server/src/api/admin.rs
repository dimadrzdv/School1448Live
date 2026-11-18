use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::public::{check_sdp, sdp_created};
use super::{ApiError, ApiResult, AppState, ClientIp, RecordingDto, StreamDto};
use crate::auth::{self, Admin};
use crate::db::{self, RecordingPatch, StreamInput};
use crate::sfu::RecordingTarget;
use crate::util::short_id;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/login", post(login))
        .route("/admin/logout", post(logout))
        .route("/admin/me", get(me))
        .route("/admin/status", get(status))
        .route("/admin/streams", get(list_streams).post(create_stream))
        .route("/admin/streams/{id}", patch(update_stream).delete(delete_stream))
        .route("/admin/streams/{id}/end", post(end_stream))
        .route("/admin/streams/{id}/whip", post(whip))
        .route("/admin/streams/{id}/poster", post(upload_poster))
        .route("/admin/whip/{session}", delete(whip_delete))
        .route("/admin/recordings", get(list_recordings))
        .route("/admin/recordings/{id}", patch(update_recording).delete(delete_recording))
}

#[derive(Deserialize)]
struct LoginBody {
    code: String,
}

async fn login(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    auth::check_origin(&headers, &s.config.allowed_origins)?;
    if let Some(wait) = s.limiter.check(ip) {
        return Err(ApiError::TooManyAttempts(wait));
    }
    let code = body.code.trim().to_string();
    let hash = s.config.admin_code_hash.clone();
    // Argon2 is deliberately slow: keep it off the async workers.
    let ok = tokio::task::spawn_blocking(move || auth::verify_code(&hash, &code))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    if !ok {
        s.limiter.fail(ip);
        tracing::warn!(%ip, "неверный код администрации");
        return Err(ApiError::Forbidden("Неверный код"));
    }
    s.limiter.success(ip);
    let _ = db::purge_sessions(&s.db).await;
    let cookie = auth::create_session(&s.db, s.config.cookie_secure).await?;
    tracing::info!(%ip, "вход администрации");
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

async fn logout(State(s): State<AppState>, admin: Admin) -> ApiResult<Response> {
    db::delete_session(&s.db, &admin.token_hash).await?;
    let cookie = auth::session_cookie("", 0, s.config.cookie_secure);
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

async fn me(_admin: Admin) -> Json<Value> {
    Json(json!({ "admin": true }))
}

async fn status(State(s): State<AppState>, _admin: Admin) -> ApiResult<Json<Value>> {
    let rooms = s.sfu.stats().await;
    let jobs = db::pending_job_count(&s.db).await?;
    Ok(Json(json!({
        "rooms": rooms,
        "pendingJobs": jobs,
        "egressLimitBps": s.config.media.egress_limit_bps,
    })))
}

async fn list_streams(State(s): State<AppState>, _admin: Admin) -> ApiResult<Json<Vec<StreamDto>>> {
    let streams = db::admin_streams(&s.db).await?;
    Ok(Json(streams.into_iter().map(|st| StreamDto::new(st, &s.hub, None)).collect()))
}

fn validate(input: &StreamInput) -> ApiResult<()> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(ApiError::BadRequest("Название: от 1 до 120 символов".into()));
    }
    if input.description.chars().count() > 2000 {
        return Err(ApiError::BadRequest("Описание слишком длинное".into()));
    }
    if !db::valid_category(&input.category) {
        return Err(ApiError::BadRequest("Неизвестная категория".into()));
    }
    Ok(())
}

async fn create_stream(
    State(s): State<AppState>,
    _admin: Admin,
    Json(input): Json<StreamInput>,
) -> ApiResult<(StatusCode, Json<StreamDto>)> {
    validate(&input)?;
    let stream = db::create_stream(&s.db, &input).await?;
    s.hub.streams_changed();
    Ok((StatusCode::CREATED, Json(StreamDto::new(stream, &s.hub, None))))
}

async fn update_stream(
    State(s): State<AppState>,
    _admin: Admin,
    Path(id): Path<String>,
    Json(input): Json<StreamInput>,
) -> ApiResult<Json<StreamDto>> {
    validate(&input)?;
    let stream = db::update_stream(&s.db, &id, &input).await?.ok_or(ApiError::NotFound)?;
    s.hub.streams_changed();
    Ok(Json(StreamDto::new(stream, &s.hub, None)))
}

async fn delete_stream(State(s): State<AppState>, _admin: Admin, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if !db::delete_stream(&s.db, &id).await? {
        return Err(ApiError::Conflict("Удалить можно только запланированный эфир".into()));
    }
    let _ = tokio::fs::remove_file(s.config.public_dir().join("posters").join(format!("{id}.jpg"))).await;
    s.hub.streams_changed();
    Ok(StatusCode::NO_CONTENT)
}

async fn end_stream(State(s): State<AppState>, _admin: Admin, Path(id): Path<String>) -> ApiResult<StatusCode> {
    db::get_stream(&s.db, &id).await?.ok_or(ApiError::NotFound)?;
    // The SFU closes the room; the event handler updates the database and starts processing.
    s.sfu.end_room(id);
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct WhipQuery {
    reclaim: Option<String>,
    #[serde(default)]
    takeover: bool,
}

/// WHIP: the studio page on the phone publishes its camera.
async fn whip(
    State(s): State<AppState>,
    _admin: Admin,
    Path(id): Path<String>,
    Query(q): Query<WhipQuery>,
    headers: HeaderMap,
    body: String,
) -> ApiResult<Response> {
    check_sdp(&headers, &body)?;
    let stream = db::get_stream(&s.db, &id).await?.ok_or(ApiError::NotFound)?;
    if stream.status == "ended" {
        return Err(ApiError::Conflict("Этот эфир уже завершён. Создайте новый.".into()));
    }
    let new_rec = short_id(12);
    let target = RecordingTarget { recording_id: new_rec.clone(), dir: s.config.raw_dir().join(&new_rec) };
    let answer = s.sfu.publish(id.clone(), body, Some(target), q.reclaim, q.takeover).await?;
    let rec_id = db::start_stream(&s.db, &stream, &new_rec).await?;
    if rec_id != new_rec {
        tracing::debug!(recording = %rec_id, "продолжение существующей записи");
    }
    s.hub.set_publisher(&id, false);
    s.hub.streams_changed();
    Ok(sdp_created(answer.sdp, format!("/api/admin/whip/{}", answer.session)))
}

async fn whip_delete(State(s): State<AppState>, _admin: Admin, Path(session): Path<String>) -> StatusCode {
    s.sfu.close_session(session);
    StatusCode::NO_CONTENT
}

async fn upload_poster(
    State(s): State<AppState>,
    _admin: Admin,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    if body.len() > 1024 * 1024 || !body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err(ApiError::BadRequest("Ожидается JPEG до 1 МБ".into()));
    }
    db::get_stream(&s.db, &id).await?.ok_or(ApiError::NotFound)?;
    let dir = s.config.public_dir().join("posters");
    tokio::fs::create_dir_all(&dir).await.map_err(anyhow::Error::from)?;
    let tmp = dir.join(format!("{id}.jpg.tmp"));
    tokio::fs::write(&tmp, &body).await.map_err(anyhow::Error::from)?;
    tokio::fs::rename(&tmp, dir.join(format!("{id}.jpg"))).await.map_err(anyhow::Error::from)?;
    let first = db::get_stream(&s.db, &id).await?.is_some_and(|st| st.poster_at.is_none());
    db::set_poster(&s.db, &id).await?;
    if first {
        s.hub.streams_changed();
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_recordings(
    State(s): State<AppState>,
    _admin: Admin,
    Query(q): Query<db::RecordingQuery>,
) -> ApiResult<Json<Value>> {
    let (items, total) = db::list_recordings(&s.db, &q, true).await?;
    let items: Vec<RecordingDto> = items.into_iter().map(Into::into).collect();
    Ok(Json(json!({ "items": items, "total": total })))
}

async fn update_recording(
    State(s): State<AppState>,
    _admin: Admin,
    Path(id): Path<String>,
    Json(patch): Json<RecordingPatch>,
) -> ApiResult<Json<RecordingDto>> {
    if let Some(t) = &patch.title
        && (t.trim().is_empty() || t.chars().count() > 120) {
            return Err(ApiError::BadRequest("Название: от 1 до 120 символов".into()));
        }
    if let Some(c) = &patch.category
        && !db::valid_category(c) {
            return Err(ApiError::BadRequest("Неизвестная категория".into()));
        }
    let rec = db::patch_recording(&s.db, &id, &patch).await?.ok_or(ApiError::NotFound)?;
    s.hub.archive_changed();
    Ok(Json(rec.into()))
}

async fn delete_recording(State(s): State<AppState>, _admin: Admin, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if !db::delete_recording(&s.db, &id).await? {
        return Err(ApiError::Conflict("Запись ещё идёт или не найдена".into()));
    }
    let _ = tokio::fs::remove_dir_all(s.config.public_dir().join("rec").join(&id)).await;
    let _ = tokio::fs::remove_dir_all(s.config.raw_dir().join(&id)).await;
    s.hub.archive_changed();
    Ok(StatusCode::NO_CONTENT)
}
