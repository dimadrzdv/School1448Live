//! Post-processing of recordings with ffmpeg, one job at a time at low priority.
//!
//! `finalize` makes a recording playable as soon as possible: a single-part H.264
//! recording is only remuxed into HLS (seconds). Anything else (VP8, codec or
//! resolution changes) is transcoded. `ladder` then adds lower qualities.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tokio::sync::Notify;

use super::recorder::{PartMeta, RecordingMeta};
use crate::db::{self, Db, Rendition};
use crate::hub::Hub;

const MAX_ATTEMPTS: i64 = 3;

#[derive(Clone)]
pub struct Jobs {
    db: Db,
    raw_dir: PathBuf,
    public_dir: PathBuf,
    hub: Arc<Hub>,
    pub notify: Arc<Notify>,
    ffmpeg: String,
    ffprobe: String,
}

impl Jobs {
    pub fn new(db: Db, raw_dir: PathBuf, public_dir: PathBuf, hub: Arc<Hub>) -> Self {
        Jobs {
            db,
            raw_dir,
            public_dir,
            hub,
            notify: Arc::new(Notify::new()),
            ffmpeg: std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
            ffprobe: std::env::var("FFPROBE").unwrap_or_else(|_| "ffprobe".into()),
        }
    }

    pub fn spawn(&self) {
        let this = self.clone();
        tokio::spawn(async move { this.run().await });
    }

    pub async fn enqueue_finalize(&self, recording_id: &str) -> Result<()> {
        db::set_recording_status(&self.db, recording_id, "processing", None).await?;
        db::enqueue_job(&self.db, recording_id, "finalize").await?;
        self.notify.notify_one();
        Ok(())
    }

    async fn run(self) {
        loop {
            match db::claim_job(&self.db).await {
                Ok(Some(job)) => {
                    tracing::info!(recording = %job.recording_id, kind = %job.kind, "обработка записи");
                    let result = match job.kind.as_str() {
                        "finalize" => self.finalize(&job.recording_id).await,
                        "ladder" => self.ladder(&job.recording_id).await,
                        other => Err(anyhow::anyhow!("unknown job kind {other}")),
                    };
                    match result {
                        Ok(()) => {
                            let _ = db::finish_job(&self.db, job.id, None, false).await;
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            tracing::error!(recording = %job.recording_id, "ошибка обработки: {msg}");
                            let retry = job.attempts < MAX_ATTEMPTS;
                            let _ = db::finish_job(&self.db, job.id, Some(&msg), retry).await;
                            if !retry && job.kind == "finalize" {
                                let _ = db::set_recording_status(&self.db, &job.recording_id, "failed", Some(&msg)).await;
                            }
                        }
                    }
                    self.hub.archive_changed();
                }
                Ok(None) => {
                    let _ = tokio::time::timeout(Duration::from_secs(60), self.notify.notified()).await;
                }
                Err(e) => {
                    tracing::error!("очередь задач: {e:#}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        let raw = self.raw_dir.join(id);
        let mut parts: Vec<PartMeta> = match RecordingMeta::load(&raw) {
            Some(m) => m.parts,
            None => self.scan_parts(&raw).await,
        };
        parts.retain(|p| raw.join(&p.file).metadata().is_ok_and(|m| m.len() > 1024));
        if parts.is_empty() {
            // Nothing was recorded (the camera never connected): no archive entry.
            tracing::info!(recording = %id, "запись пуста, удаляю");
            sqlx::query("DELETE FROM recordings WHERE id = ?").bind(id).execute(&self.db).await?;
            let _ = tokio::fs::remove_dir_all(&raw).await;
            return Ok(());
        }

        let out = self.public_dir.join("rec").join(id);
        let _ = tokio::fs::remove_dir_all(&out).await;
        tokio::fs::create_dir_all(out.join("v0")).await?;

        let copyable = parts.len() == 1 && parts[0].codec == "h264";
        let mut source = None;
        if copyable {
            match self.remux(&raw.join(&parts[0].file), &out.join("v0")).await {
                Ok(()) => {
                    let p = &parts[0];
                    source = Some((p.width, p.height, p.codec_string.clone().unwrap_or_else(|| "avc1.42e01f".into())));
                }
                Err(e) => {
                    tracing::warn!("remux не удался, перекодирую: {e:#}");
                    let _ = tokio::fs::remove_dir_all(out.join("v0")).await;
                    tokio::fs::create_dir_all(out.join("v0")).await?;
                }
            }
        }
        let (width, height, codec) = match source {
            Some(s) => s,
            None => {
                let max_h = parts.iter().map(|p| p.height.max(p.width.min(p.height))).max().unwrap_or(720);
                let h = if max_h >= 1000 { 1080 } else { 720 };
                self.transcode_parts(&raw, &parts, h, &out.join("v0")).await?;
                (h * 16 / 9, h, high_profile_codec(h).to_string())
            }
        };

        let (duration_ms, avg, peak) = playlist_stats(&out.join("v0")).await?;
        let has_thumb = self.thumbnail(&out, duration_ms).await.is_ok();
        let renditions =
            vec![Rendition { dir: "v0".into(), codec, width, height, bandwidth: avg.max(1) }];
        write_master(&out, &[(renditions[0].clone(), peak)]).await?;
        db::set_recording_media(&self.db, id, duration_ms as i64, width, height, &renditions, has_thumb).await?;
        db::set_recording_status(&self.db, id, "ready", None).await?;
        db::enqueue_job(&self.db, id, "ladder").await?;
        // The HLS copy is now the source of truth.
        if std::env::var("KEEP_RAW").ok().as_deref() != Some("1") {
            let _ = tokio::fs::remove_dir_all(&raw).await;
        }
        tracing::info!(recording = %id, duration_s = duration_ms / 1000, "запись готова к просмотру");
        Ok(())
    }

    async fn ladder(&self, id: &str) -> Result<()> {
        let Some(rec) = db::get_recording(&self.db, id).await? else {
            return Ok(());
        };
        let out = self.public_dir.join("rec").join(id);
        let source_height = rec.height.unwrap_or(720) as u32;
        let mut renditions: Vec<Rendition> = serde_json::from_str(&rec.renditions).unwrap_or_default();
        let Some(source) = renditions.first().cloned() else {
            return Ok(());
        };
        let targets: Vec<(u32, &str, &str)> = [(720, "2800k", "5600k"), (480, "1200k", "2400k"), (360, "700k", "1400k")]
            .into_iter()
            .filter(|(h, _, _)| *h + 60 < source_height)
            .take(2)
            .collect();

        let mut variants = vec![(source.clone(), 0)];
        for (i, (h, rate, buf)) in targets.iter().enumerate() {
            let name = format!("v{}", i + 1);
            let dir = out.join(&name);
            let _ = tokio::fs::remove_dir_all(&dir).await;
            tokio::fs::create_dir_all(&dir).await?;
            let vf = format!("scale=-2:{h}:flags=bicubic,setsar=1");
            let mut args = vec!["-i".to_string(), out.join("v0/index.m3u8").display().to_string()];
            args.extend(
                [
                    "-map", "0:v:0", "-map", "0:a:0?", "-vf", &vf, "-c:v", "libx264", "-preset", "veryfast",
                    "-profile:v", "high", "-crf", "23", "-maxrate", rate, "-bufsize", buf, "-g", "60",
                    "-keyint_min", "60", "-sc_threshold", "0", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k",
                ]
                .map(String::from),
            );
            args.extend(hls_args(&dir));
            self.ffmpeg(&args).await.with_context(|| format!("{h}p"))?;
            let (_, avg, peak) = playlist_stats(&dir).await?;
            let width = ((h * 16 / 9) + 1) & !1;
            variants.push((
                Rendition { dir: name, codec: high_profile_codec(*h).into(), width, height: *h, bandwidth: avg },
                peak,
            ));
        }
        // Peak of the source rendition for the master playlist.
        let (_, _, peak0) = playlist_stats(&out.join("v0")).await?;
        variants[0].1 = peak0;
        write_master(&out, &variants).await?;
        renditions = variants.into_iter().map(|(r, _)| r).collect();
        db::set_recording_media(
            &self.db,
            id,
            rec.duration_ms.unwrap_or(0),
            rec.width.unwrap_or(0) as u32,
            source_height,
            &renditions,
            rec.has_thumb,
        )
        .await?;
        Ok(())
    }

    async fn remux(&self, input: &Path, dir: &Path) -> Result<()> {
        let mut args = vec!["-i".to_string(), input.display().to_string()];
        args.extend(
            [
                "-map", "0:v:0", "-map", "0:a:0?", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-ac", "2",
                "-af", "aresample=async=1000",
            ]
            .map(String::from),
        );
        args.extend(hls_args(dir));
        self.ffmpeg(&args).await
    }

    /// Normalizes every part to the same canvas and codec, then joins them.
    /// Each intermediate is cut to exactly its video duration (audio padded with
    /// silence), so joining cannot drift or truncate; the result is verified.
    async fn transcode_parts(&self, raw: &Path, parts: &[PartMeta], height: u32, dir: &Path) -> Result<()> {
        let width = height * 16 / 9;
        let tmp = raw.join("tmp");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await?;
        let mut list = String::new();
        let mut expected = 0.0f64;
        for (i, part) in parts.iter().enumerate() {
            let input = raw.join(&part.file);
            let duration = self.video_duration(&input).await.unwrap_or(0.0);
            if duration < 0.1 {
                tracing::warn!(file = %part.file, "часть без видео пропущена");
                continue;
            }
            expected += duration;
            let out = tmp.join(format!("p{i:03}.mp4"));
            let has_audio = self.has_audio(&input).await;
            let video = format!(
                "[0:v]scale={width}:{height}:force_original_aspect_ratio=decrease,\
                 pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30,setpts=PTS-STARTPTS[v]"
            );
            let graph = if has_audio {
                format!("{video};[0:a]aresample=48000:async=1000,asetpts=PTS-STARTPTS,apad[a]")
            } else {
                format!("{video};[1:a]anull[a]")
            };
            let mut args = vec!["-i".to_string(), input.display().to_string()];
            if !has_audio {
                args.extend(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"].map(String::from));
            }
            args.extend(
                [
                    "-filter_complex", &graph, "-map", "[v]", "-map", "[a]", "-c:v", "libx264", "-preset",
                    "veryfast", "-profile:v", "high", "-crf", "21", "-maxrate", "6M", "-bufsize", "12M", "-g", "60",
                    "-keyint_min", "60", "-sc_threshold", "0", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k",
                    "-ar", "48000", "-ac", "2", "-t",
                ]
                .map(String::from),
            );
            args.push(format!("{duration:.3}"));
            args.push(out.display().to_string());
            self.ffmpeg(&args).await.with_context(|| format!("перекодирование {}", part.file))?;
            list.push_str(&format!("file '{}'\n", out.display().to_string().replace('\'', "'\\''")));
        }
        if list.is_empty() {
            bail!("ни в одной части нет видео");
        }
        let list_path = tmp.join("list.txt");
        tokio::fs::write(&list_path, list).await?;
        let mut args = ["-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
        args.push(list_path.display().to_string());
        args.extend(["-c", "copy"].map(String::from));
        args.extend(hls_args(dir));
        let result = self.ffmpeg(&args).await;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        result?;
        let (got_ms, _, _) = playlist_stats(dir).await?;
        let got = got_ms as f64 / 1000.0;
        if (got - expected).abs() > 2.0 + expected * 0.02 {
            bail!("длительность после склейки {got:.1} с, ожидалось {expected:.1} с");
        }
        Ok(())
    }

    async fn video_duration(&self, input: &Path) -> Option<f64> {
        // Last video packet time; live Matroska files have no duration header.
        let out = Command::new(&self.ffprobe)
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time", "-of", "csv=p=0"])
            .arg(input)
            .output()
            .await
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let times: Vec<f64> = text.lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
        let first = times.iter().cloned().fold(f64::INFINITY, f64::min);
        let last = times.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (times.len() > 1).then(|| last - first + 1.0 / 30.0)
    }

    async fn thumbnail(&self, out: &Path, duration_ms: u64) -> Result<()> {
        let at = ((duration_ms as f64 / 1000.0) * 0.1).clamp(0.0, 30.0);
        let mut args = vec!["-ss".to_string(), format!("{at:.2}")];
        args.extend(["-i".to_string(), out.join("v0/index.m3u8").display().to_string()]);
        args.extend(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "4"].map(String::from));
        args.push(out.join("thumb.jpg").display().to_string());
        self.ffmpeg(&args).await
    }

    /// Rebuilds part info from the files when meta.json is missing (hard crash).
    async fn scan_parts(&self, raw: &Path) -> Vec<PartMeta> {
        let mut files = Vec::new();
        if let Ok(mut dir) = tokio::fs::read_dir(raw).await {
            while let Ok(Some(e)) = dir.next_entry().await {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("part-") && name.ends_with(".mkv") {
                    files.push(name);
                }
            }
        }
        files.sort();
        let mut parts = Vec::new();
        for file in files {
            let out = Command::new(&self.ffprobe)
                .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height", "-of", "csv=p=0"])
                .arg(raw.join(&file))
                .output()
                .await;
            let Ok(out) = out else { continue };
            let text = String::from_utf8_lossy(&out.stdout);
            let mut f = text.trim().split(',');
            let (Some(codec), Some(w), Some(h)) = (f.next(), f.next(), f.next()) else { continue };
            parts.push(PartMeta {
                file,
                codec: codec.to_string(),
                codec_string: None,
                width: w.parse().unwrap_or(0),
                height: h.parse().unwrap_or(0),
                first_ms: 0,
                last_ms: 0,
            });
        }
        parts
    }

    async fn has_audio(&self, input: &Path) -> bool {
        let output = Command::new(&self.ffprobe)
            .args(["-v", "error", "-select_streams", "a", "-count_packets", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0"])
            .arg(input)
            .output()
            .await;
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().unwrap_or(0) > 0,
            Err(_) => false,
        }
    }

    async fn ffmpeg(&self, args: &[String]) -> Result<()> {
        let mut cmd = if cfg!(unix) {
            let mut c = Command::new("nice");
            c.args(["-n", "10"]).arg(&self.ffmpeg);
            c
        } else {
            Command::new(&self.ffmpeg)
        };
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = cmd.output().await.context("ffmpeg не запустился (установлен ли ffmpeg?)")?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            bail!("ffmpeg: {tail}");
        }
        Ok(())
    }
}

fn hls_args(dir: &Path) -> Vec<String> {
    let mut v = [
        "-f", "hls", "-hls_time", "4", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
        "-hls_fmp4_init_filename", "init.mp4", "-hls_flags", "independent_segments", "-hls_segment_filename",
    ]
    .map(String::from)
    .to_vec();
    v.push(dir.join("seg_%05d.m4s").display().to_string());
    v.push(dir.join("index.m3u8").display().to_string());
    v
}

fn high_profile_codec(height: u32) -> &'static str {
    match height {
        h if h >= 1000 => "avc1.640028",
        h if h >= 700 => "avc1.64001f",
        _ => "avc1.64001e",
    }
}

/// Duration (ms), average and peak bitrate of an HLS rendition.
async fn playlist_stats(dir: &Path) -> Result<(u64, u64, u64)> {
    let text = tokio::fs::read_to_string(dir.join("index.m3u8")).await?;
    let mut total_s = 0.0f64;
    let mut total_bytes = 0u64;
    let mut peak = 0u64;
    let mut pending: Option<f64> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending = rest.split(',').next().and_then(|d| d.parse().ok());
        } else if !line.starts_with('#') && !line.trim().is_empty()
            && let Some(d) = pending.take() {
                let size = tokio::fs::metadata(dir.join(line.trim())).await.map(|m| m.len()).unwrap_or(0);
                total_s += d;
                total_bytes += size;
                if d > 0.5 {
                    peak = peak.max((size as f64 * 8.0 / d) as u64);
                }
            }
    }
    if total_s <= 0.0 {
        bail!("пустой плейлист");
    }
    let avg = (total_bytes as f64 * 8.0 / total_s) as u64;
    Ok(((total_s * 1000.0) as u64, avg, peak.max(avg)))
}

async fn write_master(out: &Path, variants: &[(Rendition, u64)]) -> Result<()> {
    let mut sorted: Vec<&(Rendition, u64)> = variants.iter().collect();
    sorted.sort_by_key(|(r, _)| r.bandwidth);
    let mut m = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    for (r, peak) in sorted {
        m.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={},RESOLUTION={}x{},CODECS=\"{},mp4a.40.2\"\n{}/index.m3u8\n",
            (*peak).max(r.bandwidth),
            r.bandwidth,
            r.width,
            r.height,
            r.codec,
            r.dir
        ));
    }
    let tmp = out.join("master.m3u8.tmp");
    tokio::fs::write(&tmp, m).await?;
    tokio::fs::rename(tmp, out.join("master.m3u8")).await?;
    Ok(())
}
