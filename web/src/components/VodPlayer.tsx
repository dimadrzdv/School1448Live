import { useEffect, useRef, useState } from 'react'
import { Icon } from '../lib/icons'

interface Level {
  index: number
  height: number
}

/** Archive player: native HLS on Safari/iOS, hls.js (loaded on demand) elsewhere. */
export function VodPlayer({ src, poster, onPlay }: { src: string; poster?: string | null; onPlay?: () => void }) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const hlsRef = useRef<any>(null)
  const [levels, setLevels] = useState<Level[]>([])
  const [current, setCurrent] = useState(-1)
  const [menu, setMenu] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const played = useRef(false)

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    let destroyed = false
    setError(null)
    setLevels([])

    const nativeHls = video.canPlayType('application/vnd.apple.mpegurl') !== ''
    const isApple = /iPad|iPhone|iPod|Macintosh/.test(navigator.userAgent) && 'ontouchend' in document
    if (nativeHls && (isApple || !('MediaSource' in window))) {
      video.src = src
      return
    }

    void import('hls.js/light').then(({ default: Hls }) => {
      if (destroyed) return
      if (!Hls.isSupported()) {
        video.src = src
        return
      }
      const hls = new Hls({
        // Start near HD instead of the lowest rung; ABR corrects within a segment.
        abrEwmaDefaultEstimate: 3_000_000,
        startLevel: -1,
        capLevelToPlayerSize: true,
        maxBufferLength: 30,
        enableWorker: true,
      })
      hlsRef.current = hls
      hls.on(Hls.Events.MANIFEST_PARSED, (_e: unknown, data: { levels: { height: number }[] }) => {
        setLevels(
          data.levels
            .map((l, index) => ({ index, height: l.height }))
            .sort((a, b) => b.height - a.height),
        )
      })
      hls.on(Hls.Events.LEVEL_SWITCHED, (_e: unknown, data: { level: number }) => {
        if (hls.autoLevelEnabled) setCurrent(-1)
        else setCurrent(data.level)
      })
      hls.on(Hls.Events.ERROR, (_e: unknown, data: { fatal: boolean; type: string }) => {
        if (!data.fatal) return
        if (data.type === Hls.ErrorTypes.NETWORK_ERROR) hls.startLoad()
        else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError()
        else setError('Не удалось воспроизвести запись')
      })
      hls.loadSource(src)
      hls.attachMedia(video)
    })

    return () => {
      destroyed = true
      hlsRef.current?.destroy()
      hlsRef.current = null
    }
  }, [src])

  const choose = (index: number) => {
    const hls = hlsRef.current
    if (hls) {
      hls.currentLevel = index
      setCurrent(index)
    }
    setMenu(false)
  }

  return (
    <div className="player">
      <video
        ref={videoRef}
        controls
        playsInline
        preload="metadata"
        poster={poster ?? undefined}
        onPlay={() => {
          if (!played.current) {
            played.current = true
            onPlay?.()
          }
        }}
      />
      {error && (
        <div className="player-overlay">
          <div>
            <Icon name="alert" />
            <p>{error}</p>
          </div>
        </div>
      )}
      {levels.length > 1 && (
        <div className="quality" style={{ position: 'absolute', top: 12, right: 12, zIndex: 5 }}>
          <button className="quality-btn" onClick={() => setMenu((m) => !m)} aria-expanded={menu}>
            {current === -1 ? 'Авто' : `${levels.find((l) => l.index === current)?.height ?? ''}p`}
          </button>
          {menu && (
            <div className="quality-menu" role="menu" style={{ top: 40, bottom: 'auto' }}>
              <button role="menuitemradio" aria-checked={current === -1} onClick={() => choose(-1)}>
                Авто {current === -1 && <Icon name="check" />}
              </button>
              {levels.map((l) => (
                <button
                  key={l.index}
                  role="menuitemradio"
                  aria-checked={current === l.index}
                  onClick={() => choose(l.index)}
                >
                  {l.height}p {current === l.index && <Icon name="check" />}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
