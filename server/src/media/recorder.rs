//! Writes the live stream to disk on its own thread so the SFU loop never blocks on I/O.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use super::mkv::{self, MkvWriter, TRACK_AUDIO, TRACK_VIDEO, VideoCodec, VideoTrack};
use crate::sfu::SfuEvent;
use crate::util::now_ms;

pub enum RecMsg {
    Video { codec: VideoCodec, data: Arc<[u8]>, ts_ms: u64, keyframe: bool },
    Audio { data: Arc<[u8]>, ts_ms: u64 },
    Finish,
}

pub struct RecorderHandle {
    tx: SyncSender<RecMsg>,
    dropped: u64,
}

impl RecorderHandle {
    pub fn start(
        stream_id: String,
        recording_id: String,
        dir: PathBuf,
        events: UnboundedSender<SfuEvent>,
    ) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        // ~10 s of frames at 30 fps video + 50 fps audio.
        let (tx, rx) = mpsc::sync_channel(1024);
        thread::Builder::new()
            .name(format!("rec-{recording_id}"))
            .spawn(move || {
                let ok = Recorder::new(dir).run(rx);
                let _ = events.send(SfuEvent::RecordingFinished { stream_id, recording_id, ok });
            })?;
        Ok(RecorderHandle { tx, dropped: 0 })
    }

    pub fn send(&mut self, msg: RecMsg) {
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped += 1;
                if self.dropped.is_power_of_two() {
                    tracing::warn!(dropped = self.dropped, "запись не успевает за потоком (медленный диск?)");
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn finish(self) {
        // Blocking send: the final message must not be dropped.
        let _ = self.tx.send(RecMsg::Finish);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartMeta {
    pub file: String,
    pub codec: String,
    pub codec_string: Option<String>,
    pub width: u32,
    pub height: u32,
    pub first_ms: u64,
    pub last_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMeta {
    pub parts: Vec<PartMeta>,
    pub started_at: i64,
    pub ended_at: i64,
}

impl RecordingMeta {
    pub fn load(dir: &Path) -> Option<Self> {
        let data = fs::read(dir.join("meta.json")).ok()?;
        serde_json::from_slice(&data).ok()
    }
}

struct OpenPart {
    writer: MkvWriter<BufWriter<File>>,
    meta: PartMeta,
    /// SPS (H.264) or frame size (VP8) the part was opened with.
    signature: Vec<u8>,
    codec: VideoCodec,
}

struct Recorder {
    dir: PathBuf,
    meta: RecordingMeta,
    part: Option<OpenPart>,
    failed: bool,
    last_saved: std::time::Instant,
}

impl Recorder {
    fn new(dir: PathBuf) -> Self {
        // Continue an existing recording (server restarted during a broadcast).
        let mut meta = RecordingMeta::load(&dir).unwrap_or_default();
        if meta.started_at == 0 {
            meta.started_at = now_ms();
        }
        Recorder { dir, meta, part: None, failed: false, last_saved: std::time::Instant::now() }
    }

    fn run(mut self, rx: Receiver<RecMsg>) -> bool {
        while let Ok(msg) = rx.recv() {
            match msg {
                RecMsg::Video { codec, data, ts_ms, keyframe } => self.video(codec, &data, ts_ms, keyframe),
                RecMsg::Audio { data, ts_ms } => self.audio(&data, ts_ms),
                RecMsg::Finish => break,
            }
        }
        self.close_part();
        self.meta.ended_at = now_ms();
        self.save_meta();
        !self.failed && !self.meta.parts.is_empty()
    }

    fn video(&mut self, codec: VideoCodec, data: &[u8], ts_ms: u64, keyframe: bool) {
        if self.failed {
            return;
        }
        if keyframe
            && let Some((signature, track)) = describe_keyframe(codec, data) {
                let same = self.part.as_ref().is_some_and(|p| p.codec == codec && p.signature == signature);
                if !same {
                    self.close_part();
                    self.open_part(codec, signature, track, ts_ms);
                }
            }
        let Some(part) = self.part.as_mut() else {
            return; // wait for the first decodable keyframe
        };
        if part.codec != codec {
            return;
        }
        let payload = match codec {
            VideoCodec::H264 => mkv::annexb_to_avcc(data),
            VideoCodec::Vp8 => data.to_vec(),
        };
        part.meta.last_ms = ts_ms;
        if let Err(e) = part.writer.write_frame(TRACK_VIDEO, ts_ms, keyframe, &payload) {
            self.fail(e);
        }
        // Keep meta.json current so a crash loses at most a few seconds.
        if self.last_saved.elapsed() >= std::time::Duration::from_secs(5) {
            self.save_meta();
        }
    }

    fn audio(&mut self, data: &[u8], ts_ms: u64) {
        if self.failed {
            return;
        }
        if let Some(part) = self.part.as_mut()
            && let Err(e) = part.writer.write_frame(TRACK_AUDIO, ts_ms, true, data) {
                self.fail(e);
            }
    }

    fn open_part(&mut self, codec: VideoCodec, signature: Vec<u8>, track: VideoTrack, ts_ms: u64) {
        let index = self.meta.parts.len() + 1;
        let file = format!("part-{index:03}.mkv");
        let path = self.dir.join(&file);
        let result = File::create(&path)
            .map(|f| BufWriter::with_capacity(1 << 20, f))
            .and_then(|w| MkvWriter::new(w, &track, 2));
        match result {
            Ok(writer) => {
                let codec_string = match codec {
                    VideoCodec::H264 => mkv::parse_sps(&signature).map(|s| s.codec_string()),
                    VideoCodec::Vp8 => Some("vp8".into()),
                };
                tracing::info!(file, width = track.width, height = track.height, "запись: новая часть");
                self.part = Some(OpenPart {
                    writer,
                    meta: PartMeta {
                        file,
                        codec: match codec {
                            VideoCodec::H264 => "h264".into(),
                            VideoCodec::Vp8 => "vp8".into(),
                        },
                        codec_string,
                        width: track.width,
                        height: track.height,
                        first_ms: ts_ms,
                        last_ms: ts_ms,
                    },
                    signature,
                    codec,
                });
                self.save_meta();
            }
            Err(e) => self.fail(e),
        }
    }

    fn close_part(&mut self) {
        if let Some(part) = self.part.take() {
            let meta = part.meta;
            match part.writer.finish().and_then(|w| w.into_inner().map_err(|e| e.into_error())) {
                Ok(file) => {
                    let _ = file.sync_all();
                }
                Err(e) => self.fail(e),
            }
            self.meta.parts.push(meta);
            self.save_meta();
        }
    }

    fn save_meta(&mut self) {
        self.last_saved = std::time::Instant::now();
        let mut meta = self.meta.clone();
        if let Some(open) = &self.part {
            meta.parts.push(open.meta.clone());
        }
        if let Ok(json) = serde_json::to_vec_pretty(&meta) {
            let tmp = self.dir.join("meta.json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(tmp, self.dir.join("meta.json"));
            }
        }
    }

    fn fail(&mut self, e: std::io::Error) {
        tracing::error!("ошибка записи на диск: {e}");
        self.failed = true;
    }
}

/// For a keyframe: the identity of its decoder configuration plus the track header.
fn describe_keyframe(codec: VideoCodec, data: &[u8]) -> Option<(Vec<u8>, VideoTrack)> {
    match codec {
        VideoCodec::H264 => {
            let nalus = mkv::annexb_nalus(data);
            let sps = nalus.iter().find(|n| !n.is_empty() && n[0] & 0x1F == 7)?;
            let pps = nalus.iter().find(|n| !n.is_empty() && n[0] & 0x1F == 8)?;
            let info = mkv::parse_sps(sps)?;
            Some((
                sps.to_vec(),
                VideoTrack {
                    codec,
                    width: info.width,
                    height: info.height,
                    codec_private: mkv::avcc_record(sps, pps),
                },
            ))
        }
        VideoCodec::Vp8 => {
            let (w, h) = mkv::vp8_size(data)?;
            let mut sig = w.to_be_bytes().to_vec();
            sig.extend_from_slice(&h.to_be_bytes());
            Some((sig, VideoTrack { codec, width: w, height: h, codec_private: Vec::new() }))
        }
    }
}
