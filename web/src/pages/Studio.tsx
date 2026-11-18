import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import { api } from '../api/client'
import type { StreamInfo } from '../api/types'
import { formatBitrate, formatDuration } from '../lib/format'
import { Icon } from '../lib/icons'
import { useViewerCount } from '../lib/live'
import { type PublishState, type PublishStats, PublishError, type Quality, WhipPublisher } from '../live/whip'
import '../styles/studio.css'

type Facing = 'environment' | 'user'

const QUALITY_KEY = 'sl.quality'

function readQuality(): Quality {
  try {
    return localStorage.getItem(QUALITY_KEY) === '720' ? '720' : '1080'
  } catch {
    return '1080'
  }
}

function videoConstraints(facing: Facing, quality: Quality): MediaTrackConstraints {
  const [w, h] = quality === '1080' ? [1920, 1080] : [1280, 720]
  return { facingMode: { ideal: facing }, width: { ideal: w }, height: { ideal: h }, frameRate: { ideal: 30, max: 30 } }
}

function audioConstraints(music: boolean): MediaTrackConstraints {
  return music
    ? { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: { ideal: 2 } }
    : { echoCancellation: true, noiseSuppression: true, autoGainControl: true }
}

export function Studio() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const videoRef = useRef<HTMLVideoElement>(null)
  const streamRef = useRef<MediaStream | null>(null)
  const publisherRef = useRef<WhipPublisher | null>(null)
  const wakeLock = useRef<WakeLockSentinel | null>(null)

  const [info, setInfo] = useState<StreamInfo | null>(null)
  const [facing, setFacing] = useState<Facing>('environment')
  const [quality, setQuality] = useState<Quality>(readQuality)
  const [music, setMusic] = useState(false)
  const [micOn, setMicOn] = useState(true)
  const [camError, setCamError] = useState<string | null>(null)
  const [camReady, setCamReady] = useState(false)
  const [state, setState] = useState<PublishState>('idle')
  const [stats, setStats] = useState<PublishStats | null>(null)
  const [error, setError] = useState<PublishError | null>(null)
  const [settings, setSettings] = useState(false)
  const [confirmStop, setConfirmStop] = useState(false)
  const [ended, setEnded] = useState<number | null>(null)
  const [liveSince, setLiveSince] = useState<number | null>(null)
  const [now, setNow] = useState(Date.now())
  const [portrait, setPortrait] = useState(false)
  const [hintClosed, setHintClosed] = useState(false)
  const viewers = useViewerCount(id, info?.viewers ?? 0)
  const onAir = state === 'live' || state === 'reconnecting' || state === 'connecting'

  // Admin only.
  useEffect(() => {
    api.admin.me().catch(() => navigate('/admin', { replace: true }))
    api
      .stream(id)
      .then((s) => {
        setInfo(s)
        setMusic(s.category === 'concert')
        if (s.startedAt && s.status !== 'scheduled') setLiveSince(s.startedAt)
        document.title = `Студия: ${s.title}`
      })
      .catch(() => navigate('/admin', { replace: true }))
  }, [id, navigate])

  // Camera preview.
  const openCamera = useCallback(
    async (f: Facing, q: Quality, m: boolean) => {
      setCamError(null)
      try {
        const media = await navigator.mediaDevices.getUserMedia({ video: videoConstraints(f, q), audio: audioConstraints(m) })
        streamRef.current?.getTracks().forEach((t) => t.stop())
        streamRef.current = media
        if (videoRef.current) videoRef.current.srcObject = media
        media.getAudioTracks().forEach((t) => (t.enabled = micOn))
        setCamReady(true)
      } catch (e) {
        const name = (e as DOMException)?.name
        setCamError(
          name === 'NotAllowedError'
            ? 'Нет доступа к камере. Разрешите доступ к камере и микрофону в настройках браузера.'
            : !window.isSecureContext
              ? 'Камера работает только по HTTPS. Откройте сайт по защищённому адресу.'
              : 'Не удалось включить камеру.',
        )
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )

  useEffect(() => {
    if (!info || info.status === 'ended') return
    void openCamera(facing, quality, music)
    return () => {
      streamRef.current?.getTracks().forEach((t) => t.stop())
    }
    // Only on first load; later changes go through switchCamera / applySettings.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [info?.id])

  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    const mq = window.matchMedia('(orientation: portrait)')
    const onOrient = () => setPortrait(mq.matches)
    onOrient()
    mq.addEventListener('change', onOrient)
    return () => {
      window.clearInterval(t)
      mq.removeEventListener('change', onOrient)
    }
  }, [])

  // Keep the screen on and warn before leaving while on air.
  useEffect(() => {
    if (!onAir) return
    const acquire = async () => {
      try {
        wakeLock.current = await navigator.wakeLock?.request('screen')
      } catch {
        /* unsupported */
      }
    }
    void acquire()
    const onVisible = () => document.visibilityState === 'visible' && void acquire()
    const beforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault()
    }
    document.addEventListener('visibilitychange', onVisible)
    window.addEventListener('beforeunload', beforeUnload)
    return () => {
      document.removeEventListener('visibilitychange', onVisible)
      window.removeEventListener('beforeunload', beforeUnload)
      void wakeLock.current?.release().catch(() => {})
    }
  }, [onAir])

  // Poster for viewers: a snapshot every 30 s while live.
  useEffect(() => {
    if (state !== 'live') return
    const snap = () => {
      const v = videoRef.current
      if (!v || !v.videoWidth) return
      const w = 640
      const h = Math.round((v.videoHeight / v.videoWidth) * w)
      const canvas = document.createElement('canvas')
      canvas.width = w
      canvas.height = h
      canvas.getContext('2d')?.drawImage(v, 0, 0, w, h)
      canvas.toBlob((b) => b && void api.admin.uploadPoster(id, b).catch(() => {}), 'image/jpeg', 0.8)
    }
    const first = window.setTimeout(snap, 3000)
    const t = window.setInterval(snap, 30_000)
    return () => {
      window.clearTimeout(first)
      window.clearInterval(t)
    }
  }, [state, id])

  useEffect(
    () => () => {
      publisherRef.current?.stop()
    },
    [],
  )

  const goLive = async (takeover = false) => {
    const media = streamRef.current
    if (!media) return
    setError(null)
    publisherRef.current?.stop()
    const pub = new WhipPublisher(id, {
      onState: (s) => {
        setState(s)
        if (s === 'live') setLiveSince((t) => t ?? Date.now())
      },
      onStats: setStats,
      onError: setError,
    })
    publisherRef.current = pub
    await pub.start(media, { quality, music }, takeover)
  }

  const stop = async () => {
    setConfirmStop(false)
    await api.admin.endStream(id).catch(() => {})
    publisherRef.current?.stop()
    publisherRef.current = null
    streamRef.current?.getTracks().forEach((t) => t.stop())
    setEnded(liveSince ? Date.now() - liveSince : 0)
  }

  const switchCamera = async () => {
    const next: Facing = facing === 'environment' ? 'user' : 'environment'
    try {
      const media = await navigator.mediaDevices.getUserMedia({ video: videoConstraints(next, quality) })
      const track = media.getVideoTracks()[0]
      const current = streamRef.current
      if (current) {
        current.getVideoTracks().forEach((t) => {
          current.removeTrack(t)
          t.stop()
        })
        current.addTrack(track)
        if (videoRef.current) videoRef.current.srcObject = current
      }
      await publisherRef.current?.replaceVideoTrack(track)
      setFacing(next)
    } catch {
      setCamError('Не удалось переключить камеру')
    }
  }

  const toggleMic = () => {
    const on = !micOn
    streamRef.current?.getAudioTracks().forEach((t) => (t.enabled = on))
    setMicOn(on)
  }

  const applySettings = async (q: Quality, m: boolean) => {
    setQuality(q)
    setMusic(m)
    try {
      localStorage.setItem(QUALITY_KEY, q)
    } catch {
      /* ignore */
    }
    if (!onAir) await openCamera(facing, q, m)
  }

  if (ended !== null || info?.status === 'ended') {
    return (
      <div className="studio studio-done">
        <div className="studio-done-card">
          <div className="state-icon">
            <Icon name="check" />
          </div>
          <h1>Эфир завершён</h1>
          <p>
            {ended ? `Длительность: ${formatDuration(ended)}. ` : ''}Запись обрабатывается и появится в архиве через
            несколько минут.
          </p>
          <div className="studio-done-actions">
            <Link to="/admin" className="btn btn-white">
              В панель администрации
            </Link>
          </div>
        </div>
      </div>
    )
  }

  const timer = liveSince && onAir ? formatDuration(now - liveSince) : null
  const healthLabel = { good: 'Сеть: отлично', fair: 'Сеть: средне', poor: 'Сеть: слабая' }

  return (
    <div className="studio">
      <video ref={videoRef} className={facing === 'user' ? 'mirror' : ''} autoPlay playsInline muted />

      <div className="studio-top">
        <Link to="/admin" className="studio-icon-btn" aria-label="Назад">
          <Icon name="arrowLeft" />
        </Link>
        <div className="studio-title">
          <strong>{info?.title ?? '…'}</strong>
          <div className="studio-status">
            {state === 'live' && (
              <span className="badge-live">
                <span className="live-dot" />
                Эфир {timer}
              </span>
            )}
            {state === 'connecting' && <span className="studio-pill">Подключение…</span>}
            {state === 'reconnecting' && <span className="studio-pill warn">Переподключение… {timer}</span>}
            {(state === 'idle' || state === 'error' || state === 'ended') && <span className="studio-pill">Готов к эфиру</span>}
            {onAir && (
              <span className="studio-pill">
                <Icon name="eye" /> {viewers}
              </span>
            )}
          </div>
        </div>
        <button className="studio-icon-btn" onClick={() => setSettings(true)} aria-label="Настройки">
          <Icon name="settings" />
        </button>
      </div>

      {state === 'live' && stats && (
        <div className={`studio-health ${stats.health}`}>
          <span className="dot" />
          {healthLabel[stats.health]} · {stats.height ? `${stats.height}p` : ''} · {formatBitrate(stats.bitrate)}
          {stats.layers > 1 ? ` · ${stats.layers} слоя` : ''}
        </div>
      )}

      {portrait && !hintClosed && !camError && (
        <button className="studio-hint" onClick={() => setHintClosed(true)}>
          <Icon name="rotate" />
          Поверните телефон горизонтально — картинка будет шире и чётче
          <Icon name="x" />
        </button>
      )}

      {camError && (
        <div className="studio-message">
          <Icon name="alert" />
          <p>{camError}</p>
          <button className="btn btn-white btn-sm" onClick={() => void openCamera(facing, quality, music)}>
            Попробовать снова
          </button>
        </div>
      )}

      {error && (
        <div className="studio-message">
          <Icon name="alert" />
          <p>{error.message}</p>
          {error.code === 'busy' ? (
            <button className="btn btn-white btn-sm" onClick={() => void goLive(true)}>
              Перехватить эфир на это устройство
            </button>
          ) : error.code === 'auth' ? (
            <Link to="/admin" className="btn btn-white btn-sm">
              Войти заново
            </Link>
          ) : (
            <button className="btn btn-white btn-sm" onClick={() => void goLive()}>
              Повторить
            </button>
          )}
        </div>
      )}

      <div className="studio-controls">
        <button className="studio-round" onClick={() => void switchCamera()} aria-label="Сменить камеру">
          <Icon name="switchCamera" />
        </button>
        {onAir ? (
          <button className="studio-record on" onClick={() => setConfirmStop(true)} aria-label="Завершить эфир">
            <span />
          </button>
        ) : (
          <button
            className="studio-record"
            onClick={() => void goLive()}
            disabled={!!camError || !info || !camReady}
            aria-label="Начать эфир"
          >
            <span />
          </button>
        )}
        <button className={`studio-round${micOn ? '' : ' off'}`} onClick={toggleMic} aria-label={micOn ? 'Выключить микрофон' : 'Включить микрофон'}>
          <Icon name={micOn ? 'mic' : 'micOff'} />
        </button>
      </div>
      {!onAir && !camError && (
        <p className="studio-cta">{camReady ? 'Нажмите, чтобы выйти в эфир' : 'Включаем камеру…'}</p>
      )}

      {settings && (
        <div className="studio-sheet-backdrop" onClick={() => setSettings(false)}>
          <div className="studio-sheet" onClick={(e) => e.stopPropagation()} role="dialog" aria-label="Настройки эфира">
            <div className="studio-sheet-head">
              <h2>Настройки</h2>
              <button className="studio-icon-btn" onClick={() => setSettings(false)} aria-label="Закрыть">
                <Icon name="x" />
              </button>
            </div>
            <div className="setting">
              <div>
                <strong>Качество</strong>
                <small>{onAir ? 'Меняется только до начала эфира' : '1080p — для хорошего Wi-Fi или 4G/5G'}</small>
              </div>
              <div className="segmented">
                {(['1080', '720'] as Quality[]).map((q) => (
                  <button key={q} aria-pressed={quality === q} disabled={onAir} onClick={() => void applySettings(q, music)}>
                    {q}p
                  </button>
                ))}
              </div>
            </div>
            <div className="setting">
              <div>
                <strong>
                  <Icon name="music" /> Режим музыки
                </strong>
                <small>Стерео без шумоподавления — для концертов. Для речи лучше выключить.</small>
              </div>
              <button
                className={`switch${music ? ' on' : ''}`}
                role="switch"
                aria-checked={music}
                disabled={onAir}
                onClick={() => void applySettings(quality, !music)}
              >
                <span />
              </button>
            </div>
            {stats && (
              <div className="setting info">
                <small>
                  Кодек: {stats.codec.toUpperCase()} · {stats.width}×{stats.height} · {stats.fps} к/с · RTT{' '}
                  {stats.rttMs ?? '—'} мс{stats.limitation !== 'none' ? ` · ограничение: ${stats.limitation}` : ''}
                </small>
              </div>
            )}
            <p className="studio-tip">
              Совет: подключите телефон к зарядке, закрепите его на штативе и не сворачивайте браузер во время эфира.
            </p>
          </div>
        </div>
      )}

      {confirmStop && (
        <div className="studio-sheet-backdrop" onClick={() => setConfirmStop(false)}>
          <div className="studio-sheet confirm" onClick={(e) => e.stopPropagation()} role="alertdialog">
            <h2>Завершить эфир?</h2>
            <p>Зрители увидят, что трансляция окончена, а запись появится в архиве.</p>
            <div className="studio-done-actions">
              <button className="btn btn-danger" onClick={() => void stop()}>
                Завершить
              </button>
              <button className="btn btn-secondary" onClick={() => setConfirmStop(false)}>
                Продолжить эфир
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
