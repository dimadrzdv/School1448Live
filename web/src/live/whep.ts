import type { LayerPref } from '../api/types'

export type ViewerState = 'connecting' | 'playing' | 'stalled' | 'reconnecting' | 'offline'

export interface ViewerStats {
  width: number
  height: number
  fps: number
  bitrate: number
  /** Rough glass-to-glass estimate: network + jitter buffer + codec pipeline. */
  latencyMs: number | null
  /** Time from start() to the first rendered frame of this session. */
  firstFrameMs: number | null
  /** Time from page navigation to the first rendered frame (includes page load). */
  firstFramePageMs: number | null
}

interface Options {
  streamId: string
  video: HTMLVideoElement
  onState: (s: ViewerState) => void
  onStats?: (s: ViewerStats) => void
}

/**
 * Plays a live stream over WebRTC (WHEP). One HTTP round trip sets up the session;
 * the server is ICE-lite with public candidates, so there is no ICE gathering wait.
 */
export class WhepViewer {
  private pc: RTCPeerConnection | null = null
  private sessionUrl: string | null = null
  private stopped = false
  private attempt = 0
  private retryTimer: number | undefined
  private disconnectTimer: number | undefined
  private statsTimer: number | undefined
  private startedAt = 0
  private firstFrameMs: number | null = null
  private firstFramePageMs: number | null = null
  private lastFrames = 0
  private lastBytes = 0
  private lastStatsAt = 0
  private frozenFor = 0
  private state: ViewerState | null = null
  private layer: LayerPref = 'auto'
  private readonly opts: Options

  constructor(opts: Options) {
    this.opts = opts
  }

  start() {
    this.stopped = false
    this.startedAt = performance.now()
    void this.connect()
    this.statsTimer = window.setInterval(() => void this.pollStats(), 1000)
  }

  stop() {
    this.stopped = true
    window.clearTimeout(this.retryTimer)
    window.clearTimeout(this.disconnectTimer)
    window.clearInterval(this.statsTimer)
    this.closeSession()
    this.pc?.close()
    this.pc = null
  }

  setLayer(layer: LayerPref) {
    this.layer = layer
    if (this.sessionUrl) {
      void fetch(`${this.sessionUrl}/layer`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ layer }),
      }).catch(() => {})
    }
  }

  /** Called when the page learns (via SSE) that the stream is live again. */
  kick() {
    if (this.state === 'offline' || this.state === 'reconnecting') {
      window.clearTimeout(this.retryTimer)
      void this.connect()
    }
  }

  private setState(s: ViewerState) {
    if (this.state !== s) {
      this.state = s
      this.opts.onState(s)
    }
  }

  private async connect() {
    if (this.stopped) return
    window.clearTimeout(this.retryTimer)
    this.closeSession()
    this.pc?.close()
    this.setState(this.attempt === 0 ? 'connecting' : 'reconnecting')

    const pc = new RTCPeerConnection({ bundlePolicy: 'max-bundle' })
    this.pc = pc
    pc.addTransceiver('video', { direction: 'recvonly' })
    pc.addTransceiver('audio', { direction: 'recvonly' })

    const media = new MediaStream()
    pc.ontrack = (e) => {
      media.addTrack(e.track)
      const video = this.opts.video
      if (video.srcObject !== media) {
        video.srcObject = media
        video.play().catch(() => {
          /* autoplay with sound blocked: the UI starts muted */
        })
        this.watchFirstFrame(video)
      }
    }
    pc.onconnectionstatechange = () => {
      if (pc !== this.pc) return
      const s = pc.connectionState
      if (s === 'connected') {
        window.clearTimeout(this.disconnectTimer)
      } else if (s === 'disconnected') {
        // Short blips recover by themselves (Wi-Fi roaming); give them a moment.
        window.clearTimeout(this.disconnectTimer)
        this.disconnectTimer = window.setTimeout(() => {
          if (pc === this.pc && pc.connectionState !== 'connected') this.scheduleRetry()
        }, 2500)
      } else if (s === 'failed') {
        this.scheduleRetry()
      }
    }

    try {
      const offer = await pc.createOffer()
      await pc.setLocalDescription(offer)
      const res = await fetch(`/api/streams/${encodeURIComponent(this.opts.streamId)}/whep`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/sdp' },
        body: pc.localDescription?.sdp ?? offer.sdp,
      })
      if (pc !== this.pc) return
      if (res.status === 404) {
        this.setState('offline')
        this.scheduleRetry(5000)
        return
      }
      if (!res.ok) throw new Error(`WHEP ${res.status}`)
      this.sessionUrl = res.headers.get('Location')
      const answer = await res.text()
      await pc.setRemoteDescription({ type: 'answer', sdp: answer })
      if (this.layer !== 'auto') this.setLayer(this.layer)
    } catch (e) {
      if (pc === this.pc) {
        console.warn('[whep]', e)
        this.scheduleRetry()
      }
    }
  }

  /** Exact time of the first painted frame (stats polling only has 1 s resolution). */
  private watchFirstFrame(video: HTMLVideoElement) {
    const mark = () => {
      if (this.firstFrameMs === null) {
        this.firstFrameMs = Math.round(performance.now() - this.startedAt)
        this.firstFramePageMs = Math.round(performance.now())
        console.info(`[whep] первый кадр через ${this.firstFrameMs} мс`)
        this.attempt = 0
        this.setState('playing')
      }
    }
    const v = video as HTMLVideoElement & { requestVideoFrameCallback?: (cb: () => void) => number }
    if (v.requestVideoFrameCallback) v.requestVideoFrameCallback(mark)
    else video.addEventListener('loadeddata', mark, { once: true })
  }

  private scheduleRetry(delay?: number) {
    if (this.stopped) return
    this.attempt++
    if (this.state !== 'offline') this.setState('reconnecting')
    const backoff = delay ?? Math.min(500 * 2 ** Math.min(this.attempt - 1, 3), 5000)
    window.clearTimeout(this.retryTimer)
    this.retryTimer = window.setTimeout(() => void this.connect(), backoff)
  }

  private closeSession() {
    if (this.sessionUrl) {
      void fetch(this.sessionUrl, { method: 'DELETE', keepalive: true }).catch(() => {})
      this.sessionUrl = null
    }
  }

  private async pollStats() {
    const pc = this.pc
    if (!pc || this.stopped) return
    let report: RTCStatsReport
    try {
      report = await pc.getStats()
    } catch {
      return
    }
    let video: any = null
    let rtt: number | null = null
    report.forEach((s: any) => {
      if (s.type === 'inbound-rtp' && s.kind === 'video') video = s
      if (s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded' && s.currentRoundTripTime != null) {
        rtt = s.currentRoundTripTime * 1000
      }
    })
    if (!video) return
    const now = performance.now()
    const frames = video.framesDecoded ?? 0
    const bytes = video.bytesReceived ?? 0
    const dt = this.lastStatsAt ? (now - this.lastStatsAt) / 1000 : 0
    const bitrate = dt > 0 ? ((bytes - this.lastBytes) * 8) / dt : 0
    const advanced = frames > this.lastFrames
    this.lastFrames = frames
    this.lastBytes = bytes
    this.lastStatsAt = now

    if (advanced) {
      this.frozenFor = 0
      this.attempt = 0
      this.setState('playing')
    } else if (this.state === 'playing') {
      this.frozenFor++
      if (this.frozenFor >= 3) this.setState('stalled')
    }

    const jb =
      video.jitterBufferEmittedCount > 0 ? (video.jitterBufferDelay / video.jitterBufferEmittedCount) * 1000 : null
    const latencyMs = jb !== null && rtt !== null ? Math.round(rtt / 2 + jb + 60) : null
    const stats: ViewerStats = {
      width: video.frameWidth ?? 0,
      height: video.frameHeight ?? 0,
      fps: Math.round(video.framesPerSecond ?? 0),
      bitrate,
      latencyMs,
      firstFrameMs: this.firstFrameMs,
      firstFramePageMs: this.firstFramePageMs,
    }
    ;(window as any).__viewerStats = stats
    this.opts.onStats?.(stats)
  }
}
