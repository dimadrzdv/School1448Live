import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router'
import { ApiError, api } from '../api/client'
import type { StreamInfo } from '../api/types'
import { CategoryChip, Countdown, StateCard } from '../components/Cards'
import { LivePlayer } from '../components/LivePlayer'
import { formatTime, formatWhen, viewersLabel } from '../lib/format'
import { Icon } from '../lib/icons'
import { useLive, useRefreshOnFocus, useViewerCount } from '../lib/live'

export function Watch() {
  const { id = '' } = useParams()
  const { streamsVersion } = useLive()
  useRefreshOnFocus()
  const [stream, setStream] = useState<StreamInfo | null>(null)
  const [missing, setMissing] = useState(false)
  const viewers = useViewerCount(id, stream?.viewers ?? 0)

  useEffect(() => {
    let cancelled = false
    api
      .stream(id)
      .then((s) => {
        if (!cancelled) {
          setStream(s)
          setMissing(false)
          document.title = `${s.title} · Школа 1448`
        }
      })
      .catch((e) => {
        if (!cancelled && e instanceof ApiError && e.status === 404) setMissing(true)
      })
    return () => {
      cancelled = true
    }
  }, [id, streamsVersion])

  if (missing) {
    return (
      <div className="container section">
        <StateCard icon="alert" title="Эфир не найден" text="Возможно, ссылка устарела или эфир был удалён.">
          <Link to="/" className="btn btn-primary">
            На главную
          </Link>
        </StateCard>
      </div>
    )
  }

  const live = stream && (stream.status === 'live' || stream.status === 'reconnecting')

  return (
    <div className="watch">
      <div className="container">
        {!stream ? (
          <div className="skeleton" style={{ aspectRatio: '16 / 9', borderRadius: 20 }} />
        ) : live ? (
          <LivePlayer stream={stream} />
        ) : stream.status === 'scheduled' ? (
          <section className="hero" style={{ marginTop: 0 }}>
            <div className="hero-inner" style={{ gridTemplateColumns: '1fr' }}>
              <div>
                <span className="hero-eyebrow">
                  <Icon name="clock" />
                  {stream.scheduledAt ? `Начало ${formatWhen(stream.scheduledAt)}` : 'Время уточняется'}
                </span>
                <h1 style={{ fontSize: 'clamp(28px, 4vw, 44px)' }}>{stream.title}</h1>
                <p className="hero-lead">Оставьте страницу открытой — трансляция запустится сама, когда начнётся эфир.</p>
                {stream.scheduledAt && stream.scheduledAt > Date.now() && (
                  <div style={{ maxWidth: 420 }}>
                    <Countdown target={stream.scheduledAt} />
                  </div>
                )}
              </div>
            </div>
          </section>
        ) : (
          <StateCard
            icon="archive"
            title="Эфир завершён"
            text={
              stream.recordingId
                ? 'Запись уже доступна в архиве.'
                : 'Запись обрабатывается и скоро появится в архиве.'
            }
          >
            {stream.recordingId ? (
              <Link to={`/archive/${stream.recordingId}`} className="btn btn-primary">
                <Icon name="play" />
                Смотреть запись
              </Link>
            ) : (
              <Link to="/archive" className="btn btn-secondary">
                Открыть архив
              </Link>
            )}
          </StateCard>
        )}
      </div>

      {stream && (
        <div className="container">
          <div className="watch-info">
            <div>
              <h1>{stream.title}</h1>
              <div className="meta">
                {live && (
                  <span className="badge-live">
                    <span className="live-dot" />В эфире
                  </span>
                )}
                <CategoryChip category={stream.category} />
                {live && (
                  <span>
                    <Icon name="eye" />
                    {viewersLabel(viewers)}
                  </span>
                )}
                {live && stream.startedAt && (
                  <span>
                    <Icon name="clock" />
                    начало в {formatTime(stream.startedAt)}
                  </span>
                )}
              </div>
            </div>
          </div>
          {stream.description && <p className="watch-desc">{stream.description}</p>}
        </div>
      )}
    </div>
  )
}
