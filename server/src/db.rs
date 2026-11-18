use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, SqlitePool};

use crate::util::{now_ms, short_id};

pub type Db = SqlitePool;

/// Event tags, in the order the archive groups them.
pub const CATEGORIES: &[&str] = &["flag", "concert", "other"];

pub fn valid_category(c: &str) -> bool {
    CATEGORIES.contains(&c)
}

pub async fn connect(path: &Path) -> Result<Db> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

// ---------------------------------------------------------------------------
// Streams

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub status: String,
    pub scheduled_at: Option<i64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub created_at: i64,
    pub poster_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub category: String,
    pub scheduled_at: Option<i64>,
}

pub async fn create_stream(db: &Db, input: &StreamInput) -> Result<Stream> {
    let id = short_id(10);
    sqlx::query(
        "INSERT INTO streams (id, title, description, category, status, scheduled_at, created_at)
         VALUES (?, ?, ?, ?, 'scheduled', ?, ?)",
    )
    .bind(&id)
    .bind(input.title.trim())
    .bind(input.description.trim())
    .bind(&input.category)
    .bind(input.scheduled_at)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(get_stream(db, &id).await?.expect("just inserted"))
}

pub async fn update_stream(db: &Db, id: &str, input: &StreamInput) -> Result<Option<Stream>> {
    sqlx::query(
        "UPDATE streams SET title = ?, description = ?, category = ?, scheduled_at = ? WHERE id = ?",
    )
    .bind(input.title.trim())
    .bind(input.description.trim())
    .bind(&input.category)
    .bind(input.scheduled_at)
    .bind(id)
    .execute(db)
    .await?;
    // Keep the archive entry of a running broadcast in sync with the event.
    sqlx::query(
        "UPDATE recordings SET title = ?, description = ?, category = ?
         WHERE stream_id = ? AND status = 'recording'",
    )
    .bind(input.title.trim())
    .bind(input.description.trim())
    .bind(&input.category)
    .bind(id)
    .execute(db)
    .await?;
    get_stream(db, id).await
}

pub async fn get_stream(db: &Db, id: &str) -> Result<Option<Stream>> {
    Ok(sqlx::query_as::<_, Stream>("SELECT * FROM streams WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?)
}

/// Live and upcoming streams, live first, then by scheduled time.
pub async fn active_streams(db: &Db) -> Result<Vec<Stream>> {
    Ok(sqlx::query_as::<_, Stream>(
        "SELECT * FROM streams WHERE status IN ('live', 'scheduled')
         ORDER BY status = 'live' DESC, COALESCE(scheduled_at, created_at) ASC",
    )
    .fetch_all(db)
    .await?)
}

pub async fn admin_streams(db: &Db) -> Result<Vec<Stream>> {
    Ok(sqlx::query_as::<_, Stream>(
        "SELECT * FROM streams
         ORDER BY status = 'live' DESC, status = 'scheduled' DESC,
                  COALESCE(started_at, scheduled_at, created_at) DESC
         LIMIT 100",
    )
    .fetch_all(db)
    .await?)
}

pub async fn delete_stream(db: &Db, id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM streams WHERE id = ? AND status = 'scheduled'")
        .bind(id)
        .execute(db)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Marks a stream live and opens its archive entry (`new_id` unless one is already
/// recording). Returns the recording id.
pub async fn start_stream(db: &Db, stream: &Stream, new_id: &str) -> Result<String> {
    let mut tx = db.begin().await?;
    let now = now_ms();
    sqlx::query("UPDATE streams SET status = 'live', started_at = COALESCE(started_at, ?) WHERE id = ?")
        .bind(now)
        .bind(&stream.id)
        .execute(&mut *tx)
        .await?;
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM recordings WHERE stream_id = ? AND status = 'recording'",
    )
    .bind(&stream.id)
    .fetch_optional(&mut *tx)
    .await?;
    let rec_id = match existing {
        Some((id,)) => id,
        None => {
            let id = new_id.to_string();
            sqlx::query(
                "INSERT INTO recordings (id, stream_id, title, description, category, status, recorded_at, created_at)
                 VALUES (?, ?, ?, ?, ?, 'recording', ?, ?)",
            )
            .bind(&id)
            .bind(&stream.id)
            .bind(&stream.title)
            .bind(&stream.description)
            .bind(&stream.category)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    tx.commit().await?;
    Ok(rec_id)
}

pub async fn end_stream(db: &Db, id: &str) -> Result<()> {
    sqlx::query("UPDATE streams SET status = 'ended', ended_at = ? WHERE id = ? AND status = 'live'")
        .bind(now_ms())
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_poster(db: &Db, id: &str) -> Result<()> {
    sqlx::query("UPDATE streams SET poster_at = ? WHERE id = ?")
        .bind(now_ms())
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Recordings

#[derive(Debug, Clone, FromRow)]
pub struct Recording {
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
    pub renditions: String,
    pub has_thumb: bool,
    pub views: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rendition {
    /// Directory under the recording folder, e.g. `v0`.
    pub dir: String,
    /// RFC 6381 video codec string for the master playlist.
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub bandwidth: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct RecordingQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub year: Option<i32>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list_recordings(
    db: &Db,
    query: &RecordingQuery,
    include_hidden: bool,
) -> Result<(Vec<Recording>, i64)> {
    macro_rules! filter {
        () => {
            " WHERE (?6 = 1 OR (hidden = 0 AND status = 'ready'))
              AND (?1 IS NULL OR lower(title) LIKE ?1 OR title LIKE ?2 OR lower(description) LIKE ?1)
              AND (?3 IS NULL OR category = ?3)
              AND (?4 IS NULL OR (recorded_at >= ?4 AND recorded_at < ?5))"
        };
    }
    const LIST_SQL: &str =
        concat!("SELECT * FROM recordings", filter!(), " ORDER BY recorded_at DESC LIMIT ?7 OFFSET ?8");
    const COUNT_SQL: &str = concat!("SELECT COUNT(*) FROM recordings", filter!());

    // SQLite's lower() folds only ASCII; matching the lowercase and capitalized
    // forms covers typical Cyrillic searches ("концерт" finds "Концерт ...").
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s.to_lowercase().replace(['%', '_'], "")));
    let capitalized = search.as_ref().map(|s| capitalize_first(s));
    let category = query.category.clone().filter(|c| valid_category(c));
    let (from, to) = match query.year.and_then(year_bounds) {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    let limit = query.limit.unwrap_or(24).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);

    let items = sqlx::query_as::<_, Recording>(LIST_SQL)
        .bind(&search)
        .bind(&capitalized)
        .bind(&category)
        .bind(from)
        .bind(to)
        .bind(include_hidden as i64)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;
    let (total,): (i64,) = sqlx::query_as(COUNT_SQL)
        .bind(&search)
        .bind(&capitalized)
        .bind(&category)
        .bind(from)
        .bind(to)
        .bind(include_hidden as i64)
        .fetch_one(db)
        .await?;
    Ok((items, total))
}

fn capitalize_first(s: &str) -> String {
    // "%концерт%" -> "%Концерт%": catches titles that start with the word.
    let mut chars: Vec<char> = s.chars().collect();
    if let Some(i) = chars.iter().position(|c| *c != '%') {
        let upper: Vec<char> = chars[i].to_uppercase().collect();
        chars.splice(i..=i, upper);
    }
    chars.into_iter().collect()
}

fn year_bounds(year: i32) -> Option<(i64, i64)> {
    use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};
    let msk = UtcOffset::from_hms(3, 0, 0).ok()?;
    let start = PrimitiveDateTime::new(Date::from_calendar_date(year, Month::January, 1).ok()?, Time::MIDNIGHT)
        .assume_offset(msk);
    let end = PrimitiveDateTime::new(Date::from_calendar_date(year + 1, Month::January, 1).ok()?, Time::MIDNIGHT)
        .assume_offset(msk);
    Some((
        (start.unix_timestamp_nanos() / 1_000_000) as i64,
        (end.unix_timestamp_nanos() / 1_000_000) as i64,
    ))
}

pub async fn get_recording(db: &Db, id: &str) -> Result<Option<Recording>> {
    Ok(sqlx::query_as::<_, Recording>("SELECT * FROM recordings WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?)
}

pub async fn recording_years(db: &Db) -> Result<Vec<i32>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT CAST(strftime('%Y', recorded_at / 1000 + 10800, 'unixepoch') AS INTEGER)
         FROM recordings WHERE hidden = 0 AND status = 'ready' ORDER BY 1 DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(y,)| y as i32).collect())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub hidden: Option<bool>,
}

pub async fn patch_recording(db: &Db, id: &str, p: &RecordingPatch) -> Result<Option<Recording>> {
    sqlx::query(
        "UPDATE recordings SET
            title = COALESCE(?, title),
            description = COALESCE(?, description),
            category = COALESCE(?, category),
            hidden = COALESCE(?, hidden)
         WHERE id = ?",
    )
    .bind(p.title.as_deref().map(str::trim))
    .bind(p.description.as_deref().map(str::trim))
    .bind(&p.category)
    .bind(p.hidden)
    .bind(id)
    .execute(db)
    .await?;
    get_recording(db, id).await
}

pub async fn delete_recording(db: &Db, id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM recordings WHERE id = ? AND status != 'recording'")
        .bind(id)
        .execute(db)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn add_view(db: &Db, id: &str) -> Result<()> {
    sqlx::query("UPDATE recordings SET views = views + 1 WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_recording_status(db: &Db, id: &str, status: &str, error: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE recordings SET status = ?, error = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_recording_media(
    db: &Db,
    id: &str,
    duration_ms: i64,
    width: u32,
    height: u32,
    renditions: &[Rendition],
    has_thumb: bool,
) -> Result<()> {
    sqlx::query(
        "UPDATE recordings SET duration_ms = ?, width = ?, height = ?, renditions = ?, has_thumb = ?
         WHERE id = ?",
    )
    .bind(duration_ms)
    .bind(width as i64)
    .bind(height as i64)
    .bind(serde_json::to_string(renditions)?)
    .bind(has_thumb)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Jobs

#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: i64,
    pub recording_id: String,
    pub kind: String,
    pub attempts: i64,
}

pub async fn enqueue_job(db: &Db, recording_id: &str, kind: &str) -> Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO jobs (recording_id, kind, status, created_at, updated_at)
         VALUES (?, ?, 'pending', ?, ?)",
    )
    .bind(recording_id)
    .bind(kind)
    .bind(now)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Takes the oldest pending job (finalize jobs first: they make recordings playable).
pub async fn claim_job(db: &Db) -> Result<Option<Job>> {
    let job = sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = ?
         WHERE id = (SELECT id FROM jobs WHERE status = 'pending'
                     ORDER BY kind = 'finalize' DESC, id ASC LIMIT 1)
         RETURNING id, recording_id, kind, attempts",
    )
    .bind(now_ms())
    .fetch_optional(db)
    .await?;
    Ok(job)
}

pub async fn finish_job(db: &Db, id: i64, error: Option<&str>, retry: bool) -> Result<()> {
    let status = match (error, retry) {
        (None, _) => "done",
        (Some(_), true) => "pending",
        (Some(_), false) => "failed",
    };
    sqlx::query("UPDATE jobs SET status = ?, error = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(now_ms())
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn pending_job_count(db: &Db) -> Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM jobs WHERE status IN ('pending', 'running')")
            .fetch_one(db)
            .await?;
    Ok(n)
}

/// After a crash: running jobs go back to the queue, live streams end, open recordings get finalized.
pub async fn recover_after_restart(db: &Db) -> Result<Vec<String>> {
    sqlx::query("UPDATE jobs SET status = 'pending' WHERE status = 'running'")
        .execute(db)
        .await?;
    sqlx::query("UPDATE streams SET status = 'ended', ended_at = ? WHERE status = 'live'")
        .bind(now_ms())
        .execute(db)
        .await?;
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM recordings WHERE status = 'recording'")
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

// ---------------------------------------------------------------------------
// Admin sessions

pub async fn insert_session(db: &Db, token_hash: &str, expires_at: i64) -> Result<()> {
    sqlx::query("INSERT INTO admin_sessions (token_hash, created_at, expires_at) VALUES (?, ?, ?)")
        .bind(token_hash)
        .bind(now_ms())
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn session_valid(db: &Db, token_hash: &str) -> Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT expires_at FROM admin_sessions WHERE token_hash = ?")
            .bind(token_hash)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some_and(|(exp,)| exp > now_ms()))
}

pub async fn delete_session(db: &Db, token_hash: &str) -> Result<()> {
    sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn purge_sessions(db: &Db) -> Result<()> {
    sqlx::query("DELETE FROM admin_sessions WHERE expires_at < ?")
        .bind(now_ms())
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalizes_search() {
        assert_eq!(capitalize_first("%концерт%"), "%Концерт%");
    }

    #[test]
    fn year_bounds_are_moscow_time() {
        let (a, b) = year_bounds(2026).unwrap();
        // 2026-01-01T00:00+03:00 = 2025-12-31T21:00Z
        assert_eq!(a, 1_767_214_800_000);
        assert!(b > a);
    }
}
