//! Selective forwarding unit: one publisher (phone) per stream, many viewers.
//!
//! Runs on a dedicated thread. The HTTP layer talks to it through [`SfuHandle`];
//! state changes come back as [`SfuEvent`]s.

mod engine;
pub mod layers;
mod net;
pub mod rewrite;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::thread;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

pub use layers::LayerPref;

use crate::config::MediaConfig;

#[derive(Debug)]
pub struct RecordingTarget {
    pub recording_id: String,
    pub dir: PathBuf,
}

#[derive(Debug)]
pub struct Answer {
    pub sdp: String,
    pub session: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SfuError {
    #[error("Трансляция сейчас не идёт")]
    NotLive,
    #[error("Эфир уже ведётся с другого устройства")]
    Busy,
    #[error("Слишком много подключений с одного адреса")]
    TooManyViewers,
    #[error("Некорректное SDP-предложение: {0}")]
    BadOffer(String),
    #[error("Медиасервер недоступен")]
    Unavailable,
}

#[derive(Debug, Clone)]
pub enum SfuEvent {
    PublisherState { stream_id: String, connected: bool },
    /// `recorder`: a recording was running; `RecordingFinished` follows.
    RoomClosed { stream_id: String, recorder: bool },
    ViewerCounts(HashMap<String, u32>),
    RecordingFinished { stream_id: String, recording_id: String, ok: bool },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomStats {
    pub stream_id: String,
    pub publisher_connected: bool,
    pub viewers: u32,
    pub lan_viewers: u32,
    pub egress_bps: u64,
    pub layers: Vec<LayerInfo>,
    pub video_codec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerInfo {
    pub layer: usize,
    pub bitrate: u64,
    pub viewers: u32,
}

pub(crate) enum Command {
    Publish {
        stream_id: String,
        offer: String,
        recording: Option<RecordingTarget>,
        reclaim: Option<String>,
        takeover: bool,
        reply: oneshot::Sender<Result<Answer, SfuError>>,
    },
    View {
        stream_id: String,
        offer: String,
        client_ip: IpAddr,
        reply: oneshot::Sender<Result<Answer, SfuError>>,
    },
    SetLayer { session: String, pref: LayerPref },
    CloseSession { session: String },
    EndRoom { stream_id: String },
    Stats { reply: oneshot::Sender<Vec<RoomStats>> },
    Shutdown { reply: oneshot::Sender<()> },
}

#[derive(Clone)]
pub struct SfuHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl SfuHandle {
    pub fn spawn(cfg: MediaConfig, events: mpsc::UnboundedSender<SfuEvent>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        thread::Builder::new().name("sfu".into()).spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("sfu runtime");
            rt.block_on(async move {
                match engine::Engine::new(cfg, rx, events).await {
                    Ok(engine) => {
                        let _ = ready_tx.send(Ok(()));
                        engine.run().await;
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            });
        })?;
        ready_rx.recv()??;
        Ok(SfuHandle { tx })
    }

    pub async fn publish(
        &self,
        stream_id: String,
        offer: String,
        recording: Option<RecordingTarget>,
        reclaim: Option<String>,
        takeover: bool,
    ) -> Result<Answer, SfuError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Publish { stream_id, offer, recording, reclaim, takeover, reply })?;
        rx.await.map_err(|_| SfuError::Unavailable)?
    }

    pub async fn view(&self, stream_id: String, offer: String, client_ip: IpAddr) -> Result<Answer, SfuError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::View { stream_id, offer, client_ip, reply })?;
        rx.await.map_err(|_| SfuError::Unavailable)?
    }

    pub fn set_layer(&self, session: String, pref: LayerPref) {
        let _ = self.send(Command::SetLayer { session, pref });
    }

    pub fn close_session(&self, session: String) {
        let _ = self.send(Command::CloseSession { session });
    }

    pub fn end_room(&self, stream_id: String) {
        let _ = self.send(Command::EndRoom { stream_id });
    }

    pub async fn stats(&self) -> Vec<RoomStats> {
        let (reply, rx) = oneshot::channel();
        if self.send(Command::Stats { reply }).is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Ends every broadcast so recordings are closed cleanly.
    pub async fn shutdown(&self) {
        let (reply, rx) = oneshot::channel();
        if self.send(Command::Shutdown { reply }).is_ok() {
            let _ = rx.await;
        }
    }

    fn send(&self, cmd: Command) -> Result<(), SfuError> {
        self.tx.send(cmd).map_err(|_| SfuError::Unavailable)
    }
}
