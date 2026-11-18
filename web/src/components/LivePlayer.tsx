import { useCallback, useEffect, useRef, useState } from 'react'
import type { LayerPref, StreamInfo } from '../api/types'
import { formatBitrate } from '../lib/format'
import { Icon } from '../lib/icons'
import { useViewerCount } from '../lib/live'
import { type ViewerState, type ViewerStats, WhepViewer } from '../live/whep'

const QUALITIES: { id: LayerPref; label: string }[] = [
  { id: 'auto', label: 'Авто' },
  { id: 'high', label: 'Высокое' },
  { id: 'mid', label: 'Среднее' },
  { id: 'low', label: 'Экономия трафика' },
]

export function LivePlayer({ stream }: { stream: StreamInfo }) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const videoRef = useRef<HTMLVideoElement>(null)
  const viewerRef = useRef<WhepViewer | null>(null)
  const idleTimer = useRef<number | undefined>(undefined)
  const [state, setState] = useState<ViewerState>('connecting')
  const [stats, setStats] = useState<ViewerStats | null>(null)
  const [muted, setMuted] = useState(true)
  const [layer, setLayer] = useState<LayerPref>('auto')
  const [menu, setMenu] = useState(false)
  const [idle, setIdle] = useState(false)
  const [fullscreen, setFullscreen] = useState(false)
  const viewers = useViewerCount(stream.id, stream.viewers)

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    const viewer = new WhepViewer({ streamId: stream.id, video, onState: setState, onStats: setStats })
    viewerRef.current = viewer
    viewer.start()
    return () => viewer.stop()
  }, [stream.id])

  useEffect(() => {
    if (stream.status === 'live') viewerRef.current?.kick()
  }, [stream.status])

  useEffect(() => {
    const onFs = () => setFullscreen(document.fullscreenElement === wrapRef.current)
    document.addEventListener('fullscreenchange', onFs)
    return () => document.removeEventListener('fullscreenchange', onFs)
  }, [])

  const wake = useCallback(() => {
    setIdle(false)
    window.clearTimeout(idleTimer.current)
    idleTimer.current = window.setTimeout(() => setIdle(true), 3000)
  }, [])
  useEffect(() => () => window.clearTimeout(idleTimer.current), [])

  const unmute = () => {
    const v = videoRef.current
    if (!v) return
    v.muted = !muted
    setMuted(!muted)
    v.play().catch(() => {})
  }

  const toggleFullscreen = () => {
    const wrap = wrapRef.current
    const video = videoRef.current as (HTMLVideoElement & { webkitEnterFullscreen?: () => void }) | null
    if (document.fullscreenElement) {
      void document.exitFullscreen()
    } else if (wrap?.requestFullscreen) {
      void wrap.requestFullscreen().catch(() => video?.webkitEnterFullscreen?.())
    } else {
      video?.webkitEnterFullscreen?.() // iPhone Safari
    }
  }

  const pip = () => {
    const v = videoRef.current
    if (!v) return
    if (document.pictureInPictureElement) void document.exitPictureInPicture()
    else void v.requestPictureInPicture?.().catch(() => {})
  }

  const chooseLayer = (l: LayerPref) => {
    setLayer(l)
    setMenu(false)
    viewerRef.current?.setLayer(l)
  }

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'f' || e.key === 'а') toggleFullscreen()
    if (e.key === 'm' || e.key === 'ь') unmute()
  }

  const publisherAway = stream.status === 'reconnecting'
  let overlay: { title: string; hint?: string; spin: boolean } | null = null
  if (publisherAway && state !== 'playing')
    overlay = { title: 'Связь с камерой прервалась', hint: 'Эфир продолжится автоматически', spin: true }
  else if (state === 'connecting') overlay = { title: 'Подключаемся к эфиру…', spin: true }
  else if (state === 'reconnecting') overlay = { title: 'Восстанавливаем соединение…', spin: true }
  else if (state === 'offline') overlay = { title: 'Эфир скоро начнётся', hint: 'Видео появится автоматически', spin: false }
  else if (state === 'stalled' || (publisherAway && state !== 'playing'))
    overlay = {
      title: publisherAway ? 'Связь с камерой прервалась' : 'Сигнал временно пропал',
      hint: 'Эфир продолжится автоматически',
      spin: true,
    }

  const playing = state === 'playing'
  const pdpSupported = typeof document !== 'undefined' && 'pictureInPictureEnabled' in document

  return (
    <div
      ref={wrapRef}
      className={`player${idle && playing && !menu ? ' idle' : ''}`}
      onMouseMove={wake}
      onTouchStart={wake}
      onKeyDown={onKey}
      onDoubleClick={toggleFullscreen}
      tabIndex={0}
      aria-label={`Прямой эфир: ${stream.title}`}
    >
      {stream.posterUrl && !playing && (
        <div className="player-poster" style={{ backgroundImage: `url(${stream.posterUrl})` }} />
      )}
      <video ref={videoRef} autoPlay playsInline muted style={{ position: 'relative', zIndex: 1 }} />

      {overlay && (
        <div className="player-overlay" aria-live="polite">
          <div>
            {overlay.spin ? <div className="spinner" /> : <Icon name="clock" className="" />}
            <p>{overlay.title}</p>
            {overlay.hint && <small>{overlay.hint}</small>}
          </div>
        </div>
      )}

      {playing && muted && (
        <button className="unmute" onClick={unmute}>
          <Icon name="volumeX" />
          Включить звук
        </button>
      )}

      <div className="player-top">
        <span className="badge-live">
          <span className="live-dot" />
          Live
        </span>
        <span className="viewer-count">
          <Icon name="eye" />
          {viewers}
        </span>
      </div>

      <div className="player-bar">
        <button className="pbtn" onClick={unmute} aria-label={muted ? 'Включить звук' : 'Выключить звук'}>
          <Icon name={muted ? 'volumeX' : 'volume'} />
        </button>
        {stats?.latencyMs != null && playing && (
          <span style={{ fontSize: 13, opacity: 0.8 }}>задержка ≈ {(stats.latencyMs / 1000).toFixed(1)} с</span>
        )}
        <span className="spacer" />
        <div className="quality">
          <button
            className="quality-btn"
            onClick={() => setMenu((m) => !m)}
            aria-haspopup="menu"
            aria-expanded={menu}
          >
            {stats?.height ? `${stats.height}p` : 'HD'}
            {layer === 'auto' ? ' · авто' : ''}
          </button>
          {menu && (
            <div className="quality-menu" role="menu">
              {QUALITIES.map((q) => (
                <button key={q.id} role="menuitemradio" aria-checked={layer === q.id} onClick={() => chooseLayer(q.id)}>
                  {q.label}
                  {layer === q.id && <Icon name="check" />}
                </button>
              ))}
              {stats && stats.width > 0 && (
                <div className="info">
                  {stats.width}×{stats.height} · {stats.fps} к/с · {formatBitrate(stats.bitrate)}
                </div>
              )}
            </div>
          )}
        </div>
        {pdpSupported && (
          <button className="pbtn" onClick={pip} aria-label="Картинка в картинке">
            <Icon name="pip" />
          </button>
        )}
        <button className="pbtn" onClick={toggleFullscreen} aria-label="Во весь экран">
          <Icon name={fullscreen ? 'minimize' : 'maximize'} />
        </button>
      </div>
    </div>
  )
}
