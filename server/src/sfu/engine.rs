use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::SdpOffer;
use str0m::format::Codec;
use str0m::media::{KeyframeRequestKind, MediaData, MediaKind, MediaTime, Mid, Rid};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::rtp::{Extension, ExtensionMap};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::layers::{self, Demand, LayerPref, LayerSet};
use super::net::{Net, TcpEvent};
use super::rewrite::{TimeRewriter, VideoFwd};
use super::{Answer, Command, LayerInfo, RecordingTarget, RoomStats, SfuError, SfuEvent};
use crate::config::MediaConfig;
use crate::media::mkv::VideoCodec;
use crate::media::recorder::{RecMsg, RecorderHandle};
use crate::util::{is_local_ip, secret_token};

type PeerId = u64;

const TICK: Duration = Duration::from_millis(250);
const ALLOC_EVERY: Duration = Duration::from_millis(500);
const KEYFRAME_THROTTLE: Duration = Duration::from_millis(300);
/// A subscriber still waiting for its keyframe after this long asks again.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);
/// After a (new) publisher connects the recorder waits this long for the top layer
/// before settling for a lower one, so a takeover does not split the recording.
const RECORDER_SETTLE: Duration = Duration::from_millis(1500);
/// The recorded layer gets a keyframe at least this often (HLS segments, seeking).
const RECORD_GOP: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const COUNTS_EVERY: Duration = Duration::from_secs(2);
const FAR: Duration = Duration::from_secs(3600);

struct Peer {
    session: String,
    room: String,
    rtc: Rtc,
    role: Role,
    next_timeout: Instant,
    connected: bool,
    created: Instant,
    client_ip: IpAddr,
}

enum Role {
    Publisher { video_mid: Option<Mid> },
    Viewer(Box<Viewer>),
}

struct Viewer {
    video_mid: Option<Mid>,
    audio_mid: Option<Mid>,
    video: VideoFwd,
    audio: TimeRewriter,
    pref: LayerPref,
    bwe_bps: u64,
    lan: bool,
    upgrade_streak: u8,
    write_errors: u32,
    first_frame_at: Option<Instant>,
}

/// A viewer counts as just joined for this long after its first frame.
const FRESH_FOR: Duration = Duration::from_secs(3);

struct RecTap {
    handle: RecorderHandle,
    video: VideoFwd,
    audio: TimeRewriter,
}

struct Room {
    publisher: Option<PeerId>,
    publisher_session: Option<String>,
    lost_since: Option<Instant>,
    viewers: HashSet<PeerId>,
    layers: LayerSet,
    layer_rid: [Option<Rid>; layers::LAYERS],
    last_kf_request: [Option<Instant>; layers::LAYERS],
    video_codec: Option<Codec>,
    recorder: Option<RecTap>,
    last_alloc: Instant,
    publisher_since: Instant,
}

impl Room {
    fn new(now: Instant) -> Self {
        Room {
            publisher: None,
            publisher_session: None,
            lost_since: None,
            viewers: HashSet::new(),
            layers: LayerSet::default(),
            layer_rid: [None; layers::LAYERS],
            last_kf_request: [None; layers::LAYERS],
            video_codec: None,
            recorder: None,
            last_alloc: now,
            publisher_since: now,
        }
    }
}

pub struct Engine {
    cfg: MediaConfig,
    net: Net,
    cmd_rx: UnboundedReceiver<Command>,
    events: UnboundedSender<SfuEvent>,
    peers: HashMap<PeerId, Peer>,
    sessions: HashMap<String, PeerId>,
    routes: HashMap<(Protocol, SocketAddr), PeerId>,
    rooms: HashMap<String, Room>,
    candidates: Vec<Candidate>,
    next_id: PeerId,
    earliest: Instant,
    last_tick: Instant,
    last_counts: HashMap<String, u32>,
    last_counts_at: Instant,
}

impl Engine {
    pub async fn new(
        cfg: MediaConfig,
        cmd_rx: UnboundedReceiver<Command>,
        events: UnboundedSender<SfuEvent>,
    ) -> anyhow::Result<Self> {
        let local = SocketAddr::new(cfg.lan_ip, cfg.port);
        let net = Net::bind(cfg.port, local).await?;

        let mut candidates = vec![
            Candidate::host(local, "udp")?,
            Candidate::builder().tcp().host(local).tcptype(TcpType::Passive).build()?,
        ];
        if let Some(public) = cfg.public_ip {
            let addr = SocketAddr::new(public, cfg.port);
            candidates.push(Candidate::server_reflexive(addr, local, "udp")?);
            candidates.push(
                Candidate::builder()
                    .tcp()
                    .server_reflexive(addr, local)
                    .tcptype(TcpType::Passive)
                    .build()?,
            );
        }
        tracing::info!(
            udp_tcp_port = cfg.port,
            lan = %cfg.lan_ip,
            public = ?cfg.public_ip,
            "медиасервер запущен"
        );
        let now = Instant::now();
        Ok(Engine {
            cfg,
            net,
            cmd_rx,
            events,
            peers: HashMap::new(),
            sessions: HashMap::new(),
            routes: HashMap::new(),
            rooms: HashMap::new(),
            candidates,
            next_id: 1,
            earliest: now + FAR,
            last_tick: now,
            last_counts: HashMap::new(),
            last_counts_at: now,
        })
    }

    pub async fn run(mut self) {
        let mut buf = vec![0u8; 2048];
        loop {
            let now = Instant::now();
            self.drive_timeouts(now);
            if now.duration_since(self.last_tick) >= TICK {
                self.last_tick = now;
                self.tick(now);
            }
            let deadline = self.earliest.min(self.last_tick + TICK);

            tokio::select! {
                r = self.net.udp.recv_from(&mut buf) => match r {
                    Ok((n, source)) => {
                        self.on_packet(Protocol::Udp, source, &buf[..n]);
                        // Drain whatever else is already queued before timers and commands.
                        for _ in 0..64 {
                            match self.net.udp.try_recv_from(&mut buf) {
                                Ok((n, source)) => self.on_packet(Protocol::Udp, source, &buf[..n]),
                                Err(_) => break,
                            }
                        }
                    }
                    Err(e) => tracing::debug!("udp recv: {e}"),
                },
                Some(ev) = self.net.tcp_rx.recv() => match ev {
                    TcpEvent::Opened(addr, tx) => self.net.on_tcp_opened(addr, tx),
                    TcpEvent::Frame(addr, frame) => self.on_packet(Protocol::Tcp, addr, &frame),
                    TcpEvent::Closed(addr) => {
                        self.net.on_tcp_closed(addr);
                        self.routes.remove(&(Protocol::Tcp, addr));
                    }
                },
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(cmd) => self.on_command(cmd),
                    None => break,
                },
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Commands

    fn on_command(&mut self, cmd: Command) {
        match cmd {
            Command::Publish { stream_id, offer, recording, reclaim, takeover, reply } => {
                let _ = reply.send(self.publish(stream_id, &offer, recording, reclaim, takeover));
            }
            Command::View { stream_id, offer, client_ip, reply } => {
                let _ = reply.send(self.view(stream_id, &offer, client_ip));
            }
            Command::SetLayer { session, pref } => {
                if let Some(id) = self.sessions.get(&session).copied() {
                    let room = match self.peers.get_mut(&id) {
                        Some(Peer { role: Role::Viewer(v), room, .. }) => {
                            v.pref = pref;
                            v.upgrade_streak = 0;
                            room.clone()
                        }
                        _ => return,
                    };
                    self.allocate(&room, Instant::now());
                }
            }
            Command::CloseSession { session } => {
                if let Some(id) = self.sessions.get(&session).copied() {
                    if let Some(p) = self.peers.get_mut(&id) {
                        p.rtc.disconnect();
                    }
                    self.reap(Instant::now());
                }
            }
            Command::EndRoom { stream_id } => self.close_room(&stream_id),
            Command::Stats { reply } => {
                let _ = reply.send(self.stats());
            }
            Command::Shutdown { reply } => {
                let ids: Vec<String> = self.rooms.keys().cloned().collect();
                for id in ids {
                    self.close_room(&id);
                }
                let _ = reply.send(());
            }
        }
    }

    fn new_rtc(&self, bwe: Option<Bitrate>) -> Rtc {
        // Standard extensions minus video-orientation (CVO): without it the browser
        // rotates frames itself, so recordings never come out sideways.
        let mut exts = ExtensionMap::empty();
        exts.set(1, Extension::AudioLevel);
        exts.set(2, Extension::AbsoluteSendTime);
        exts.set(3, Extension::TransportSequenceNumber);
        exts.set(4, Extension::RtpMid);
        exts.set(10, Extension::RtpStreamId);
        exts.set(11, Extension::RepairedRtpStreamId);

        let mut builder = Rtc::builder()
            .set_ice_lite(true)
            .clear_codecs()
            .enable_opus(true, false)
            .enable_h264(true)
            .enable_vp8(true)
            .set_extension_map(exts)
            .set_stats_interval(Some(Duration::from_secs(2)));
        if bwe.is_some() {
            builder = builder.enable_bwe(bwe);
        }
        let mut rtc = builder.build(Instant::now());
        for c in &self.candidates {
            rtc.add_local_candidate(c.clone());
        }
        rtc
    }

    fn add_peer(&mut self, rtc: Rtc, room: &str, role: Role, client_ip: IpAddr) -> (PeerId, String) {
        let id = self.next_id;
        self.next_id += 1;
        let session = secret_token()[..32].to_string();
        let now = Instant::now();
        self.peers.insert(
            id,
            Peer {
                session: session.clone(),
                room: room.to_string(),
                rtc,
                role,
                next_timeout: now,
                connected: false,
                created: now,
                client_ip,
            },
        );
        self.sessions.insert(session.clone(), id);
        (id, session)
    }

    fn publish(
        &mut self,
        stream_id: String,
        offer: &str,
        recording: Option<RecordingTarget>,
        reclaim: Option<String>,
        takeover: bool,
    ) -> Result<Answer, SfuError> {
        if let Some(pid) = self.rooms.get(&stream_id).and_then(|r| r.publisher) {
            let room = &self.rooms[&stream_id];
            let reclaiming = reclaim.is_some() && reclaim == room.publisher_session;
            let alive = self.peers.get(&pid).is_some_and(|p| p.rtc.is_alive());
            if alive && !reclaiming && !takeover {
                return Err(SfuError::Busy);
            }
        }
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| SfuError::BadOffer(e.to_string()))?;
        let mut rtc = self.new_rtc(None);
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| SfuError::BadOffer(e.to_string()))?;

        // Replace the previous publisher (reconnect after network change, or takeover).
        let old = self.rooms.get_mut(&stream_id).and_then(|r| r.publisher.take());
        if let Some(old) = old {
            self.remove_peer(old);
        }

        let (id, session) = self.add_peer(
            rtc,
            &stream_id,
            Role::Publisher { video_mid: None },
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        );
        let now = Instant::now();
        let room = self.rooms.entry(stream_id.clone()).or_insert_with(|| Room::new(now));
        room.publisher = Some(id);
        room.publisher_session = Some(session.clone());
        room.lost_since = None;
        room.publisher_since = now;
        room.layers.reset();
        room.layer_rid = [None; layers::LAYERS];
        room.last_kf_request = [None; layers::LAYERS];
        room.video_codec = None;
        for vid in &room.viewers {
            if let Some(Peer { role: Role::Viewer(v), .. }) = self.peers.get_mut(vid) {
                v.video.reset();
                v.audio.reanchor();
            }
        }
        match (&mut room.recorder, recording) {
            (Some(rec), _) => {
                rec.video.reset();
                rec.audio.reanchor();
            }
            (None, Some(target)) => {
                match RecorderHandle::start(
                    stream_id.clone(),
                    target.recording_id,
                    target.dir,
                    self.events.clone(),
                ) {
                    Ok(handle) => {
                        room.recorder = Some(RecTap {
                            handle,
                            video: VideoFwd::new(now, false),
                            audio: TimeRewriter::new(48_000, now),
                        });
                    }
                    Err(e) => tracing::error!("не удалось начать запись: {e}"),
                }
            }
            (None, None) => {}
        }
        self.drive(id);
        tracing::info!(stream = %stream_id, "публикация: подключается телефон");
        Ok(Answer { sdp: answer.to_sdp_string(), session })
    }

    fn view(&mut self, stream_id: String, offer: &str, client_ip: IpAddr) -> Result<Answer, SfuError> {
        if !self.rooms.contains_key(&stream_id) {
            return Err(SfuError::NotLive);
        }
        let same_ip = self
            .peers
            .values()
            .filter(|p| matches!(p.role, Role::Viewer(_)) && p.client_ip == client_ip)
            .count();
        if same_ip >= self.cfg.max_viewers_per_ip && !client_ip.is_loopback() {
            return Err(SfuError::TooManyViewers);
        }
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| SfuError::BadOffer(e.to_string()))?;
        let lan = is_local_ip(client_ip);
        let initial = if lan { 5_000_000 } else { 1_500_000 };
        let mut rtc = self.new_rtc(Some(Bitrate::bps(initial)));
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| SfuError::BadOffer(e.to_string()))?;
        let now = Instant::now();
        let viewer = Viewer {
            video_mid: None,
            audio_mid: None,
            video: VideoFwd::new(now, true),
            audio: TimeRewriter::new(48_000, now),
            pref: LayerPref::Auto,
            bwe_bps: initial,
            lan,
            upgrade_streak: 0,
            write_errors: 0,
            first_frame_at: None,
        };
        let (id, session) = self.add_peer(rtc, &stream_id, Role::Viewer(Box::new(viewer)), client_ip);
        if let Some(room) = self.rooms.get_mut(&stream_id) {
            room.viewers.insert(id);
        }
        self.drive(id);
        Ok(Answer { sdp: answer.to_sdp_string(), session })
    }

    // -----------------------------------------------------------------------
    // Network input and str0m polling

    fn on_packet(&mut self, proto: Protocol, source: SocketAddr, data: &[u8]) {
        let Ok(contents) = data.try_into() else {
            return;
        };
        let input = Input::Receive(
            Instant::now(),
            Receive { proto, source, destination: self.net.local, contents },
        );
        let cached = self
            .routes
            .get(&(proto, source))
            .copied()
            .filter(|id| self.peers.get(id).is_some_and(|p| p.rtc.accepts(&input)));
        let id = match cached {
            Some(id) => id,
            None => {
                let Some(id) = self.peers.iter().find(|(_, p)| p.rtc.accepts(&input)).map(|(id, _)| *id) else {
                    return;
                };
                self.routes.insert((proto, source), id);
                id
            }
        };
        if let Some(peer) = self.peers.get_mut(&id)
            && let Err(e) = peer.rtc.handle_input(input) {
                tracing::debug!("handle_input: {e}");
                peer.rtc.disconnect();
            }
        self.drive(id);
    }

    fn drive_timeouts(&mut self, now: Instant) {
        if now < self.earliest {
            return;
        }
        let due: Vec<PeerId> =
            self.peers.iter().filter(|(_, p)| p.next_timeout <= now).map(|(id, _)| *id).collect();
        for id in due {
            if let Some(peer) = self.peers.get_mut(&id)
                && peer.rtc.is_alive()
                    && let Err(e) = peer.rtc.handle_input(Input::Timeout(now)) {
                        tracing::debug!("timeout: {e}");
                        peer.rtc.disconnect();
                    }
            self.drive(id);
        }
        self.earliest = self.peers.values().map(|p| p.next_timeout).min().unwrap_or(now + FAR);
    }

    /// Flushes a peer's output and handles its events.
    fn drive(&mut self, id: PeerId) {
        let mut events = Vec::new();
        if let Some(peer) = self.peers.get_mut(&id) {
            poll(peer, &mut self.net, &mut events);
            self.earliest = self.earliest.min(peer.next_timeout);
        }
        for e in events {
            self.on_event(id, e);
        }
    }

    fn on_event(&mut self, id: PeerId, event: Event) {
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        match event {
            Event::Connected => {
                peer.connected = true;
                let room = peer.room.clone();
                match &peer.role {
                    Role::Publisher { .. } => {
                        tracing::info!(stream = %room, "публикация: телефон в эфире");
                        let _ = self.events.send(SfuEvent::PublisherState { stream_id: room, connected: true });
                    }
                    Role::Viewer(_) => {
                        tracing::debug!(stream = %room, peer = id, ms = peer.created.elapsed().as_millis() as u64, "зритель: ICE+DTLS готовы");
                        self.allocate(&room, Instant::now())
                    }
                }
            }
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                peer.rtc.disconnect();
            }
            Event::MediaAdded(m) => match &mut peer.role {
                Role::Publisher { video_mid } => {
                    if m.kind == MediaKind::Video {
                        *video_mid = Some(m.mid);
                    }
                }
                Role::Viewer(v) => match m.kind {
                    MediaKind::Video => v.video_mid = Some(m.mid),
                    MediaKind::Audio => v.audio_mid = Some(m.mid),
                },
            },
            Event::MediaData(data) => {
                if matches!(peer.role, Role::Publisher { .. }) {
                    let room = peer.room.clone();
                    self.on_media(&room, data);
                }
            }
            Event::KeyframeRequest(_) => {
                // The viewer lost packets it could not recover: ask the phone for a keyframe.
                if let Role::Viewer(v) = &peer.role
                    && let Some(layer) = v.video.current {
                        let room = peer.room.clone();
                        self.request_keyframe(&room, layer, Instant::now());
                    }
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. })
            | Event::EgressBitrateEstimate(BweKind::Remb { estimate, .. }) => {
                if let Role::Viewer(v) = &mut peer.role {
                    v.bwe_bps = estimate.as_u64();
                }
            }
            Event::PeerStats(stats) => {
                if let (Role::Viewer(v), Some(pair)) = (&mut peer.role, stats.selected_candidate_pair) {
                    v.lan = is_local_ip(pair.remote.addr.ip());
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Forwarding

    fn on_media(&mut self, room_id: &str, data: MediaData) {
        let now = Instant::now();
        let codec = data.params.spec().codec;
        let Some(room) = self.rooms.get_mut(room_id) else {
            return;
        };
        let mut deferred: Vec<(PeerId, Event)> = Vec::new();
        let mut want_keyframe = None;
        let mut allocate_now = false;

        if codec.is_video() {
            let layer = layers::layer_of_rid(data.rid.as_deref());
            let keyframe = data.is_keyframe();
            let new_layer = !room.layers.is_active(layer, now);
            if new_layer {
                tracing::debug!(stream = %room_id, layer, keyframe, "телефон: слой активен");
            }
            room.layer_rid[layer] = data.rid;
            room.layers.on_frame(layer, data.data.len(), keyframe, now);
            allocate_now = new_layer;
            room.video_codec = Some(codec);
            if !data.contiguous && !keyframe {
                want_keyframe = Some(layer);
            }
            let input_time = data.time.numer();
            for vid in &room.viewers {
                let Some(peer) = self.peers.get_mut(vid) else { continue };
                let Role::Viewer(v) = &mut peer.role else { continue };
                if !peer.connected {
                    continue;
                }
                let was_idle = v.video.current.is_none();
                let (Some(mid), Some(out)) =
                    (v.video_mid, v.video.accept(layer, keyframe, input_time, data.network_time))
                else {
                    continue;
                };
                if was_idle {
                    v.first_frame_at = Some(now);
                    tracing::debug!(peer = *vid, layer, ms = peer.created.elapsed().as_millis() as u64, "зритель: первый кадр отправлен");
                }
                write(&mut peer.rtc, mid, &data, out, &mut v.write_errors);
                let mut evs = Vec::new();
                poll(peer, &mut self.net, &mut evs);
                deferred.extend(evs.into_iter().map(|e| (*vid, e)));
            }
            if let Some(rec) = room.recorder.as_mut() {
                let rec_codec = match codec {
                    Codec::H264 => Some(VideoCodec::H264),
                    Codec::Vp8 => Some(VideoCodec::Vp8),
                    _ => None,
                };
                if let (Some(c), Some(out)) =
                    (rec_codec, rec.video.accept(layer, keyframe, input_time, data.network_time))
                {
                    rec.handle.send(RecMsg::Video { codec: c, data: data.data.clone(), ts_ms: out / 90, keyframe });
                }
            }
        } else if codec == Codec::Opus {
            let input_time = data.time.numer();
            for vid in &room.viewers {
                let Some(peer) = self.peers.get_mut(vid) else { continue };
                let Role::Viewer(v) = &mut peer.role else { continue };
                // Audio waits until the picture starts so both begin together.
                if !peer.connected || v.video.current.is_none() {
                    continue;
                }
                let Some(mid) = v.audio_mid else { continue };
                let out = v.audio.map(input_time, data.network_time);
                write(&mut peer.rtc, mid, &data, out, &mut v.write_errors);
                let mut evs = Vec::new();
                poll(peer, &mut self.net, &mut evs);
                deferred.extend(evs.into_iter().map(|e| (*vid, e)));
            }
            if let Some(rec) = room.recorder.as_mut()
                && rec.video.current.is_some() {
                    let out = rec.audio.map(input_time, data.network_time);
                    rec.handle.send(RecMsg::Audio { data: data.data.clone(), ts_ms: out / 48 });
                }
        }

        for p in &deferred {
            if let Some(peer) = self.peers.get(&p.0) {
                self.earliest = self.earliest.min(peer.next_timeout);
            }
        }
        for (id, e) in deferred {
            self.on_event(id, e);
        }
        if let Some(layer) = want_keyframe {
            self.request_keyframe(room_id, layer, now);
        }
        if allocate_now {
            self.allocate(room_id, now);
        }
    }

    fn request_keyframe(&mut self, room_id: &str, layer: usize, now: Instant) {
        let Some(room) = self.rooms.get_mut(room_id) else { return };
        // Skip only while an earlier request is still unanswered: its keyframe will
        // reach every subscriber waiting on this layer.
        let outstanding = room.last_kf_request[layer].is_some_and(|t| {
            now.duration_since(t) < KEYFRAME_THROTTLE
                && room.layers.layers[layer].last_keyframe.is_none_or(|k| k < t)
        });
        if outstanding {
            return;
        }
        let Some(pid) = room.publisher else { return };
        let rid = room.layer_rid[layer];
        let Some(peer) = self.peers.get_mut(&pid) else { return };
        let Role::Publisher { video_mid: Some(mid) } = peer.role else { return };
        let Some(mut writer) = peer.rtc.writer(mid) else { return };
        let kind = if writer.is_request_keyframe_possible(KeyframeRequestKind::Pli) {
            KeyframeRequestKind::Pli
        } else {
            KeyframeRequestKind::Fir
        };
        match writer.request_keyframe(rid, kind) {
            Ok(()) => {
                tracing::debug!(stream = %room_id, layer, ?rid, ?kind, "запрос ключевого кадра");
                room.last_kf_request[layer] = Some(now);
            }
            Err(e) => tracing::debug!(stream = %room_id, layer, ?rid, "запрос ключевого кадра не удался: {e}"),
        }
        self.drive(pid);
    }

    // -----------------------------------------------------------------------
    // Periodic work

    fn tick(&mut self, now: Instant) {
        self.reap(now);

        let room_ids: Vec<String> = self.rooms.keys().cloned().collect();
        let grace = Duration::from_secs(self.cfg.reconnect_grace_secs);
        for id in room_ids {
            let Some(room) = self.rooms.get_mut(&id) else { continue };
            if room.lost_since.is_some_and(|t| now.duration_since(t) > grace) {
                tracing::info!(stream = %id, "телефон не вернулся, эфир завершён");
                self.close_room(&id);
                continue;
            }

            // Recording follows the best available layer and needs regular keyframes.
            let mut kf_for_recorder = None;
            let settling = now.duration_since(room.publisher_since) < RECORDER_SETTLE
                && !room.layers.is_active(layers::LAYERS - 1, now);
            if let (Some(rec), false) = (room.recorder.as_mut(), settling)
                && let Some(best) = room.layers.highest_active(now) {
                    if rec.video.want(best, now) {
                        kf_for_recorder = Some(best);
                    } else if rec.video.target_since.is_some_and(|s| now.duration_since(s) > KEYFRAME_RETRY) {
                        rec.video.target_since = Some(now);
                        kf_for_recorder = Some(best);
                    }
                    if let Some(cur) = rec.video.current {
                        let stale = room.layers.layers[cur]
                            .last_keyframe
                            .is_none_or(|t| now.duration_since(t) >= RECORD_GOP);
                        if stale && kf_for_recorder.is_none() {
                            kf_for_recorder = Some(cur);
                        }
                    }
                }
            let alloc_due = now.duration_since(room.last_alloc) >= ALLOC_EVERY;
            // Encoders sometimes drop a keyframe request; viewers still without a
            // picture ask again every tick (deduplicated by request_keyframe).
            let lowest = room.layers.active(now).first().map(|(l, _)| *l);
            let waiting = room.viewers.iter().any(|vid| {
                matches!(self.peers.get(vid), Some(Peer { role: Role::Viewer(v), connected: true, .. })
                    if v.video.current.is_none())
            });
            if let Some(layer) = kf_for_recorder {
                self.request_keyframe(&id, layer, now);
            }
            if let (true, Some(low)) = (waiting, lowest) {
                self.request_keyframe(&id, low, now);
            }
            if alloc_due {
                self.allocate(&id, now);
            }
        }

        // Viewers that never managed to connect (blocked network) are dropped.
        for p in self.peers.values_mut() {
            if !p.connected && now.duration_since(p.created) > CONNECT_TIMEOUT {
                p.rtc.disconnect();
            }
        }

        if now.duration_since(self.last_counts_at) >= COUNTS_EVERY {
            self.last_counts_at = now;
            let counts: HashMap<String, u32> = self
                .rooms
                .iter()
                .map(|(id, r)| {
                    let n = r.viewers.iter().filter(|v| self.peers.get(v).is_some_and(|p| p.connected)).count();
                    (id.clone(), n as u32)
                })
                .collect();
            if counts != self.last_counts {
                self.last_counts = counts.clone();
                let _ = self.events.send(SfuEvent::ViewerCounts(counts));
            }
        }
    }

    fn allocate(&mut self, room_id: &str, now: Instant) {
        let Some(room) = self.rooms.get_mut(room_id) else { return };
        room.last_alloc = now;
        let active = room.layers.active(now);
        let ids: Vec<PeerId> = room
            .viewers
            .iter()
            .copied()
            .filter(|id| self.peers.get(id).is_some_and(|p| p.connected))
            .collect();
        let mut demands: Vec<Demand> = ids
            .iter()
            .map(|id| match &self.peers[id].role {
                Role::Viewer(v) => Demand {
                    pref: v.pref,
                    bwe_bps: v.bwe_bps,
                    lan: v.lan,
                    current: v.video.current,
                    upgrade_streak: v.upgrade_streak,
                    fresh: v.first_frame_at.is_none_or(|t| now.duration_since(t) < FRESH_FOR),
                },
                Role::Publisher { .. } => unreachable!("only viewers are listed"),
            })
            .collect();
        let chosen = layers::allocate(&active, &mut demands, self.cfg.egress_limit_bps);
        tracing::trace!(stream = %room_id, ?active, ?chosen, "распределение слоёв");
        let top_rate = active.last().map(|(_, b)| *b).unwrap_or(3_000_000);

        let mut keyframes = HashSet::new();
        let lowest = active.first().map(|(l, _)| *l);
        for ((id, demand), layer) in ids.iter().zip(demands).zip(chosen) {
            let Some(peer) = self.peers.get_mut(id) else { continue };
            let Role::Viewer(v) = &mut peer.role else { continue };
            v.upgrade_streak = demand.upgrade_streak;
            let Some(layer) = layer else { continue };
            // No picture yet: the low layer's small keyframe arrives fastest.
            if v.video.current.is_none()
                && let Some(low) = lowest {
                    keyframes.insert(low);
                }
            if v.video.want(layer, now) {
                keyframes.insert(layer);
            } else if v.video.target == Some(layer)
                && v.video.target_since.is_some_and(|s| now.duration_since(s) > KEYFRAME_RETRY)
            {
                v.video.target_since = Some(now);
                keyframes.insert(layer);
            }
            // Let BWE probe up to the top layer while keeping the current one sustained.
            let rate = active.iter().find(|(l, _)| *l == layer).map(|(_, b)| *b).unwrap_or(0);
            peer.rtc
                .bwe()
                .set_bitrate(Bitrate::bps(rate), Bitrate::bps((top_rate as f64 * 1.2) as u64));
        }
        for layer in keyframes {
            self.request_keyframe(room_id, layer, now);
        }
    }

    /// Removes dead peers; a lost publisher starts the reconnect grace period.
    fn reap(&mut self, now: Instant) {
        let dead: Vec<PeerId> =
            self.peers.iter().filter(|(_, p)| !p.rtc.is_alive()).map(|(id, _)| *id).collect();
        for id in dead {
            let Some(peer) = self.peers.get(&id) else { continue };
            let room_id = peer.room.clone();
            let was_publisher = matches!(peer.role, Role::Publisher { .. });
            self.remove_peer(id);
            if was_publisher
                && let Some(room) = self.rooms.get_mut(&room_id)
                    && room.publisher == Some(id) {
                        room.publisher = None;
                        room.lost_since = Some(now);
                        tracing::warn!(stream = %room_id, "связь с телефоном потеряна, ждём переподключения");
                        let _ = self.events.send(SfuEvent::PublisherState { stream_id: room_id, connected: false });
                    }
        }
    }

    fn remove_peer(&mut self, id: PeerId) {
        if let Some(mut peer) = self.peers.remove(&id) {
            peer.rtc.disconnect();
            self.sessions.remove(&peer.session);
            self.routes.retain(|_, v| *v != id);
            if let Some(room) = self.rooms.get_mut(&peer.room) {
                room.viewers.remove(&id);
            }
        }
    }

    fn close_room(&mut self, room_id: &str) {
        let Some(room) = self.rooms.remove(room_id) else {
            // Nothing running (e.g. after a restart): the app still finalizes the stream.
            let _ = self.events.send(SfuEvent::RoomClosed { stream_id: room_id.to_string(), recorder: false });
            return;
        };
        for id in room.viewers.iter().copied().chain(room.publisher) {
            if let Some(mut peer) = self.peers.remove(&id) {
                peer.rtc.disconnect();
                self.sessions.remove(&peer.session);
                self.routes.retain(|_, v| *v != id);
            }
        }
        let recorder = room.recorder.is_some();
        if let Some(rec) = room.recorder {
            rec.handle.finish();
        }
        self.last_counts.remove(room_id);
        let _ = self.events.send(SfuEvent::RoomClosed { stream_id: room_id.to_string(), recorder });
    }

    fn stats(&self) -> Vec<RoomStats> {
        let now = Instant::now();
        self.rooms
            .iter()
            .map(|(id, room)| {
                let mut per_layer = [0u32; layers::LAYERS];
                let (mut viewers, mut lan_viewers, mut egress) = (0, 0, 0);
                for vid in &room.viewers {
                    if let Some(Peer { role: Role::Viewer(v), connected: true, .. }) = self.peers.get(vid) {
                        viewers += 1;
                        if v.lan {
                            lan_viewers += 1;
                        }
                        if let Some(l) = v.video.current {
                            per_layer[l] += 1;
                            if !v.lan {
                                egress += room.layers.layers[l].bitrate;
                            }
                        }
                    }
                }
                RoomStats {
                    stream_id: id.clone(),
                    publisher_connected: room
                        .publisher
                        .and_then(|p| self.peers.get(&p))
                        .is_some_and(|p| p.connected),
                    viewers,
                    lan_viewers,
                    egress_bps: egress,
                    layers: room
                        .layers
                        .active(now)
                        .into_iter()
                        .map(|(layer, bitrate)| LayerInfo { layer, bitrate, viewers: per_layer[layer] })
                        .collect(),
                    video_codec: room.video_codec.map(|c| c.to_string()),
                }
            })
            .collect()
    }
}

fn poll(peer: &mut Peer, net: &mut Net, events: &mut Vec<Event>) {
    loop {
        if !peer.rtc.is_alive() {
            peer.next_timeout = Instant::now() + FAR;
            return;
        }
        match peer.rtc.poll_output() {
            Ok(Output::Transmit(t)) => net.send(&t),
            Ok(Output::Timeout(t)) => {
                peer.next_timeout = t;
                return;
            }
            Ok(Output::Event(e)) => events.push(e),
            Err(e) => {
                tracing::debug!("poll_output: {e}");
                peer.rtc.disconnect();
            }
        }
    }
}

fn write(rtc: &mut Rtc, mid: Mid, data: &MediaData, out_time: u64, errors: &mut u32) {
    let Some(writer) = rtc.writer(mid) else { return };
    let Some(pt) = writer.match_params(data.params) else {
        *errors += 1;
        if *errors == 1 {
            tracing::warn!("зритель не поддерживает кодек {}", data.params.spec().codec);
        }
        return;
    };
    let time = MediaTime::new(out_time, data.time.frequency());
    if let Err(e) = writer.write(pt, data.network_time, time, data.data.clone()) {
        *errors += 1;
        if errors.is_power_of_two() {
            tracing::debug!("write: {e}");
        }
    }
}
