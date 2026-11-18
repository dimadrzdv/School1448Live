export type PublishState = 'idle' | 'connecting' | 'live' | 'reconnecting' | 'ended' | 'error'
export type Quality = '1080' | '720'
export type VideoCodecChoice = 'h264' | 'vp8'

export interface PublishOptions {
  quality: Quality
  music: boolean
}

export interface PublishStats {
  bitrate: number
  width: number
  height: number
  fps: number
  rttMs: number | null
  layers: number
  codec: string
  limitation: string
  health: 'good' | 'fair' | 'poor'
}

export class PublishError extends Error {
  constructor(
    message: string,
    readonly code: 'busy' | 'auth' | 'ended' | 'network' | 'other',
  ) {
    super(message)
  }
}

interface Callbacks {
  onState: (s: PublishState) => void
  onStats?: (s: PublishStats) => void
  onError?: (e: PublishError) => void
}

// Three simulcast layers. Viewers each get the one their connection can carry;
// the server never re-encodes.
const LAYERS: Record<Quality, RTCRtpEncodingParameters[]> = {
  '1080': [
    { rid: 'q', scaleResolutionDownBy: 4, maxBitrate: 350_000, maxFramerate: 20 },
    { rid: 'h', scaleResolutionDownBy: 2, maxBitrate: 1_300_000, maxFramerate: 30 },
    { rid: 'f', scaleResolutionDownBy: 1, maxBitrate: 4_500_000, maxFramerate: 30 },
  ],
  '720': [
    { rid: 'q', scaleResolutionDownBy: 4, maxBitrate: 220_000, maxFramerate: 20 },
    { rid: 'h', scaleResolutionDownBy: 2, maxBitrate: 700_000, maxFramerate: 30 },
    { rid: 'f', scaleResolutionDownBy: 1, maxBitrate: 2_500_000, maxFramerate: 30 },
  ],
}

const CODEC_KEY = 'sl.publishCodec'

export function preferredCodec(): VideoCodecChoice {
  try {
    return localStorage.getItem(CODEC_KEY) === 'vp8' ? 'vp8' : 'h264'
  } catch {
    return 'h264'
  }
}

function rememberCodec(c: VideoCodecChoice) {
  try {
    localStorage.setItem(CODEC_KEY, c)
  } catch {
    /* private mode */
  }
}

function orderCodecs(codec: VideoCodecChoice): RTCRtpCodec[] | null {
  const caps = RTCRtpSender.getCapabilities?.('video')?.codecs
  if (!caps) return null
  const mime = (c: RTCRtpCodec) => c.mimeType.toLowerCase()
  const fmtp = (c: RTCRtpCodec) => c.sdpFmtpLine ?? ''
  const h264 = caps
    .filter((c) => mime(c) === 'video/h264' && fmtp(c).includes('packetization-mode=1'))
    .sort((a, b) => rankH264(a) - rankH264(b))
  const vp8 = caps.filter((c) => mime(c) === 'video/vp8')
  const support = caps.filter((c) => ['video/rtx', 'video/red', 'video/ulpfec'].includes(mime(c)))
  if (codec === 'h264' && h264.length === 0) return null
  return codec === 'h264' ? [...h264, ...vp8, ...support] : [...vp8, ...h264, ...support]
}

function rankH264(c: RTCRtpCodec): number {
  // Constrained baseline decodes everywhere and is what phone hardware encoders do best.
  const f = c.sdpFmtpLine ?? ''
  if (f.includes('profile-level-id=42e0')) return 0
  if (f.includes('profile-level-id=4200')) return 1
  if (f.includes('profile-level-id=4d')) return 2
  return 3
}

/** Concert sound: stereo, 128 kbit/s, no discontinuous transmission. */
function mungeOpus(sdp: string, music: boolean): string {
  if (!music) return sdp
  const pt = sdp.match(/a=rtpmap:(\d+) opus\/48000/i)?.[1]
  if (!pt) return sdp
  const extra = 'stereo=1;sprop-stereo=1;maxaveragebitrate=128000;usedtx=0'
  const re = new RegExp(`a=fmtp:${pt} ([^\\r\\n]*)`)
  if (re.test(sdp)) {
    return sdp.replace(re, (_m, params: string) => {
      const kept = params
        .split(';')
        .filter((p) => !/^(stereo|sprop-stereo|maxaveragebitrate|usedtx)=/.test(p.trim()))
      return `a=fmtp:${pt} ${[...kept, extra].join(';')}`
    })
  }
  return sdp.replace(`a=rtpmap:${pt} opus/48000/2`, `a=rtpmap:${pt} opus/48000/2\r\na=fmtp:${pt} ${extra}`)
}

/**
 * Publishes the phone camera to the server (WHIP). Keeps the broadcast alive across
 * network changes by re-publishing into the same room with a reclaim token.
 */
export class WhipPublisher {
  private pc: RTCPeerConnection | null = null
  private session: string | null = null
  private stream: MediaStream | null = null
  private options: PublishOptions = { quality: '1080', music: false }
  private codec: VideoCodecChoice = preferredCodec()
  private triedFallback = false
  private stopped = true
  private attempt = 0
  private state: PublishState = 'idle'
  private retryTimer: number | undefined
  private disconnectTimer: number | undefined
  private statsTimer: number | undefined
  private healthCheckTimer: number | undefined
  private lastBytes = 0
  private lastStatsAt = 0
  private readonly streamId: string
  private readonly cb: Callbacks

  constructor(streamId: string, cb: Callbacks) {
    this.streamId = streamId
    this.cb = cb
    window.addEventListener('online', this.onOnline)
  }

  get currentCodec() {
    return this.codec
  }

  async start(stream: MediaStream, options: PublishOptions, takeover = false) {
    window.removeEventListener('online', this.onOnline)
    window.addEventListener('online', this.onOnline)
    this.stream = stream
    this.options = options
    this.stopped = false
    this.attempt = 0
    this.triedFallback = false
    await this.connect(takeover)
    window.clearInterval(this.statsTimer)
    this.statsTimer = window.setInterval(() => void this.pollStats(), 1000)
  }

  /** Switch camera without renegotiation. */
  async replaceVideoTrack(track: MediaStreamTrack) {
    track.contentHint = 'motion'
    const sender = this.pc?.getSenders().find((s) => s.track?.kind === 'video')
    await sender?.replaceTrack(track)
  }

  async replaceAudioTrack(track: MediaStreamTrack) {
    const sender = this.pc?.getSenders().find((s) => s.track?.kind === 'audio')
    await sender?.replaceTrack(track)
  }

  /** Stops publishing. The server keeps the stream "reconnecting" unless it is ended via the API. */
  stop() {
    this.stopped = true
    window.clearTimeout(this.retryTimer)
    window.clearTimeout(this.disconnectTimer)
    window.clearTimeout(this.healthCheckTimer)
    window.clearInterval(this.statsTimer)
    window.removeEventListener('online', this.onOnline)
    if (this.session) {
      void fetch(`/api/admin/whip/${this.session}`, { method: 'DELETE', keepalive: true, credentials: 'same-origin' }).catch(
        () => {},
      )
    }
    this.pc?.close()
    this.pc = null
    this.setState('ended')
  }

  private onOnline = () => {
    if (!this.stopped && this.state === 'reconnecting') {
      window.clearTimeout(this.retryTimer)
      void this.connect(false)
    }
  }

  private setState(s: PublishState) {
    if (this.state !== s) {
      this.state = s
      this.cb.onState(s)
    }
  }

  private async connect(takeover: boolean) {
    if (this.stopped || !this.stream) return
    const reclaim = this.session
    this.pc?.close()
    this.setState(reclaim ? 'reconnecting' : 'connecting')

    const pc = new RTCPeerConnection({ bundlePolicy: 'max-bundle' })
    this.pc = pc
    const video = this.stream.getVideoTracks()[0]
    const audio = this.stream.getAudioTracks()[0]
    if (video) {
      video.contentHint = 'motion'
      let tx: RTCRtpTransceiver
      try {
        tx = pc.addTransceiver(video, {
          direction: 'sendonly',
          streams: [this.stream],
          sendEncodings: LAYERS[this.options.quality].map((e) => ({ ...e })),
        })
      } catch {
        // Browser without simulcast: one layer at full quality.
        tx = pc.addTransceiver(video, {
          direction: 'sendonly',
          streams: [this.stream],
          sendEncodings: [{ maxBitrate: LAYERS[this.options.quality][2].maxBitrate, maxFramerate: 30 }],
        })
      }
      const order = orderCodecs(this.codec)
      if (order) {
        try {
          tx.setCodecPreferences(order)
        } catch {
          /* keep browser default */
        }
      } else if (this.codec === 'h264') {
        this.codec = 'vp8'
      }
    }
    if (audio) {
      const atx = pc.addTransceiver(audio, { direction: 'sendonly', streams: [this.stream] })
      if (this.options.music) {
        const params = atx.sender.getParameters()
        params.encodings = [{ ...(params.encodings?.[0] ?? {}), maxBitrate: 128_000 }]
        atx.sender.setParameters(params).catch(() => {})
      }
    }

    pc.onconnectionstatechange = () => {
      if (pc !== this.pc) return
      const s = pc.connectionState
      if (s === 'connected') {
        window.clearTimeout(this.disconnectTimer)
        this.attempt = 0
        this.setState('live')
        this.scheduleEncoderCheck(pc)
      } else if (s === 'disconnected') {
        window.clearTimeout(this.disconnectTimer)
        this.disconnectTimer = window.setTimeout(() => {
          if (pc === this.pc && pc.connectionState !== 'connected') this.retry()
        }, 2000)
      } else if (s === 'failed') {
        this.retry()
      }
    }

    try {
      const offer = await pc.createOffer()
      await pc.setLocalDescription(offer)
      const q = new URLSearchParams()
      if (reclaim) q.set('reclaim', reclaim)
      if (takeover) q.set('takeover', 'true')
      const res = await fetch(`/api/admin/streams/${encodeURIComponent(this.streamId)}/whip?${q}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/sdp' },
        body: pc.localDescription?.sdp ?? offer.sdp,
        credentials: 'same-origin',
      })
      if (pc !== this.pc) return
      if (!res.ok) {
        const body = await res.json().catch(() => ({}))
        const msg: string = body.error ?? `Ошибка ${res.status}`
        if (res.status === 409 && body.code === 'busy') throw new PublishError(msg, 'busy')
        if (res.status === 409) throw new PublishError(msg, 'ended')
        if (res.status === 401) throw new PublishError(msg, 'auth')
        throw new PublishError(msg, res.status >= 500 ? 'network' : 'other')
      }
      const location = res.headers.get('Location') ?? ''
      this.session = location.split('/').pop() || null
      const answer = mungeOpus(await res.text(), this.options.music)
      await pc.setRemoteDescription({ type: 'answer', sdp: answer })
    } catch (e) {
      if (pc !== this.pc) return
      const err = e instanceof PublishError ? e : new PublishError('Нет связи с сервером', 'network')
      if (err.code === 'network' && reclaim) {
        this.retry()
        return
      }
      this.pc?.close()
      this.pc = null
      this.stopped = true
      this.setState('error')
      this.cb.onError?.(err)
    }
  }

  private retry() {
    if (this.stopped) return
    this.attempt++
    this.setState('reconnecting')
    window.clearTimeout(this.retryTimer)
    const delay = Math.min(400 * 2 ** Math.min(this.attempt - 1, 3), 3000)
    this.retryTimer = window.setTimeout(() => void this.connect(false), delay)
  }

  /**
   * Some Android devices negotiate H.264 but never produce frames (no hardware
   * encoder for simulcast). If nothing is encoded 4 s after connecting, switch to VP8.
   */
  private scheduleEncoderCheck(pc: RTCPeerConnection) {
    window.clearTimeout(this.healthCheckTimer)
    if (this.codec !== 'h264' || this.triedFallback) return
    this.healthCheckTimer = window.setTimeout(async () => {
      if (pc !== this.pc || this.stopped) return
      const report = await pc.getStats().catch(() => null)
      let frames = 0
      report?.forEach((s: any) => {
        if (s.type === 'outbound-rtp' && s.kind === 'video') frames += s.framesEncoded ?? 0
      })
      if (frames === 0) {
        console.warn('[whip] H.264 не кодирует кадры, переключаюсь на VP8')
        this.triedFallback = true
        this.codec = 'vp8'
        rememberCodec('vp8')
        void this.connect(false)
      } else {
        rememberCodec('h264')
      }
    }, 4000)
  }

  private async pollStats() {
    const pc = this.pc
    if (!pc || this.state !== 'live') return
    const report = await pc.getStats().catch(() => null)
    if (!report) return
    let bytes = 0
    let layers = 0
    let top: any = null
    let rtt: number | null = null
    let limitation = 'none'
    const codecs = new Map<string, string>()
    report.forEach((s: any) => {
      if (s.type === 'codec') codecs.set(s.id, String(s.mimeType ?? '').replace('video/', ''))
    })
    let codec = ''
    report.forEach((s: any) => {
      if (s.type === 'outbound-rtp' && s.kind === 'video') {
        bytes += s.bytesSent ?? 0
        if (s.active !== false && (s.framesPerSecond ?? 0) > 0) layers++
        if (!top || (s.frameHeight ?? 0) > (top.frameHeight ?? 0)) top = s
        if (s.qualityLimitationReason && s.qualityLimitationReason !== 'none') limitation = s.qualityLimitationReason
        if (s.codecId && codecs.has(s.codecId)) codec = codecs.get(s.codecId)!
      }
      if (s.type === 'remote-inbound-rtp' && s.kind === 'video' && s.roundTripTime != null) {
        rtt = Math.max(rtt ?? 0, s.roundTripTime * 1000)
      }
    })
    const now = performance.now()
    const dt = this.lastStatsAt ? (now - this.lastStatsAt) / 1000 : 0
    const bitrate = dt > 0 && bytes >= this.lastBytes ? ((bytes - this.lastBytes) * 8) / dt : 0
    this.lastBytes = bytes
    this.lastStatsAt = now
    const r = rtt as number | null
    const health: PublishStats['health'] =
      limitation === 'bandwidth' || (r !== null && r > 400) ? 'poor' : limitation === 'cpu' || (r !== null && r > 150) ? 'fair' : 'good'
    this.cb.onStats?.({
      bitrate,
      width: top?.frameWidth ?? 0,
      height: top?.frameHeight ?? 0,
      fps: Math.round(top?.framesPerSecond ?? 0),
      rttMs: r === null ? null : Math.round(r),
      layers,
      codec: codec || this.codec,
      limitation,
      health,
    })
  }
}
