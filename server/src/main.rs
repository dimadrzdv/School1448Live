mod api;
mod auth;
mod config;
mod db;
mod hub;
mod media;
mod sfu;
mod util;

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use crate::api::AppState;
use crate::config::Config;
use crate::hub::Hub;
use crate::media::jobs::Jobs;
use crate::sfu::{SfuEvent, SfuHandle};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("schoollive=info,str0m=warn,tower_http=warn")),
        )
        .init();

    // `schoollive hash-code` reads the secret code from stdin and prints ADMIN_CODE_HASH.
    if std::env::args().nth(1).as_deref() == Some("hash-code") {
        let mut code = String::new();
        std::io::stdin().read_to_string(&mut code)?;
        let code = code.trim();
        anyhow::ensure!(code.chars().count() >= 6, "код должен быть не короче 6 символов");
        eprintln!("Добавьте эту строку в .env:");
        println!("ADMIN_CODE_HASH='{}'", auth::hash_code(code)?);
        return Ok(());
    }

    str0m::crypto::from_feature_flags().install_process_default();

    let config = Arc::new(Config::from_env()?);
    for dir in [config.data_dir.clone(), config.raw_dir(), config.public_dir()] {
        std::fs::create_dir_all(&dir).with_context(|| format!("создание {}", dir.display()))?;
    }
    let db = db::connect(&config.db_path()).await.context("база данных")?;
    let hub = Arc::new(Hub::default());
    let jobs = Jobs::new(db.clone(), config.raw_dir(), config.public_dir(), hub.clone());

    // Anything that was live when the server stopped is closed and its recording processed.
    for rec in db::recover_after_restart(&db).await? {
        tracing::info!(recording = %rec, "восстановление: обработка незавершённой записи");
        jobs.enqueue_finalize(&rec).await?;
    }
    jobs.spawn();

    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let sfu = SfuHandle::spawn(config.media.clone(), events_tx).context("медиасервер")?;

    let state = AppState {
        db: db.clone(),
        config: config.clone(),
        sfu,
        hub: hub.clone(),
        limiter: Default::default(),
    };
    tokio::spawn(handle_sfu_events(events_rx, db, hub, jobs));

    let sfu = state.sfu.clone();
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(config.http_addr).await.context("HTTP порт")?;
    tracing::info!("HTTP: http://{}", config.http_addr);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    // Close broadcasts so recorders flush their files; give the event handler a moment.
    sfu.shutdown().await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    tracing::info!("остановлен");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("остановка: завершаю эфиры и закрываю записи");
}

async fn handle_sfu_events(
    mut rx: mpsc::UnboundedReceiver<SfuEvent>,
    db: db::Db,
    hub: Arc<Hub>,
    jobs: Jobs,
) {
    while let Some(event) = rx.recv().await {
        let result: Result<()> = async {
            match event {
                SfuEvent::PublisherState { stream_id, connected } => {
                    hub.set_publisher(&stream_id, connected);
                    hub.streams_changed();
                }
                SfuEvent::ViewerCounts(counts) => hub.set_counts(&counts),
                SfuEvent::RoomClosed { stream_id, recorder } => {
                    db::end_stream(&db, &stream_id).await?;
                    hub.remove(&stream_id);
                    hub.streams_changed();
                    if !recorder {
                        // No recorder was running (e.g. restart): process what is on disk.
                        let open: Vec<(String,)> = sqlx::query_as(
                            "SELECT id FROM recordings WHERE stream_id = ? AND status = 'recording'",
                        )
                        .bind(&stream_id)
                        .fetch_all(&db)
                        .await?;
                        for (id,) in open {
                            jobs.enqueue_finalize(&id).await?;
                        }
                    }
                    tracing::info!(stream = %stream_id, "эфир завершён");
                }
                SfuEvent::RecordingFinished { stream_id, recording_id, ok } => {
                    // The recorder may have written under a fresh id while the database
                    // continued an older entry; process whichever entry is open.
                    let open: Vec<(String,)> = sqlx::query_as(
                        "SELECT id FROM recordings WHERE (id = ? OR stream_id = ?) AND status = 'recording'",
                    )
                    .bind(&recording_id)
                    .bind(&stream_id)
                    .fetch_all(&db)
                    .await?;
                    for (id,) in open {
                        if ok || id != recording_id {
                            jobs.enqueue_finalize(&id).await?;
                        } else {
                            db::set_recording_status(&db, &id, "failed", Some("Видео не было записано")).await?;
                        }
                    }
                    hub.archive_changed();
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::error!("обработка события медиасервера: {e:#}");
        }
    }
}
