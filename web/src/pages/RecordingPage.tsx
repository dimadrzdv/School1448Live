import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router'
import { ApiError, api } from '../api/client'
import type { Recording } from '../api/types'
import { CategoryChip, RecordingCard, StateCard } from '../components/Cards'
import { VodPlayer } from '../components/VodPlayer'
import { formatDate, formatDuration, formatTime, viewsLabel } from '../lib/format'
import { Icon } from '../lib/icons'

export function RecordingPage() {
  const { id = '' } = useParams()
  const [rec, setRec] = useState<Recording | null>(null)
  const [missing, setMissing] = useState(false)
  const [related, setRelated] = useState<Recording[]>([])

  useEffect(() => {
    let cancelled = false
    setRec(null)
    api
      .recording(id)
      .then((r) => {
        if (cancelled) return
        setRec(r)
        document.title = `${r.title} · Архив школы 1448`
        api
          .recordings({ category: r.category, limit: 4 })
          .then((p) => !cancelled && setRelated(p.items.filter((x) => x.id !== r.id).slice(0, 3)))
          .catch(() => {})
      })
      .catch((e) => {
        if (!cancelled && e instanceof ApiError && e.status === 404) setMissing(true)
      })
    return () => {
      cancelled = true
    }
  }, [id])

  if (missing) {
    return (
      <div className="container section">
        <StateCard icon="alert" title="Запись не найдена" text="Возможно, она была скрыта или удалена.">
          <Link to="/archive" className="btn btn-primary">
            В архив
          </Link>
        </StateCard>
      </div>
    )
  }

  return (
    <div className="watch">
      <div className="container">
        {rec?.playlistUrl ? (
          <VodPlayer src={rec.playlistUrl} poster={rec.thumbnailUrl} onPlay={() => api.addView(rec.id)} />
        ) : (
          <div className="skeleton" style={{ aspectRatio: '16 / 9', borderRadius: 20 }} />
        )}
      </div>
      {rec && (
        <div className="container">
          <div className="watch-info">
            <div>
              <h1>{rec.title}</h1>
              <div className="meta">
                <CategoryChip category={rec.category} />
                <span>
                  <Icon name="calendar" />
                  {formatDate(rec.recordedAt)}, {formatTime(rec.recordedAt)}
                </span>
                <span>
                  <Icon name="clock" />
                  {formatDuration(rec.durationMs)}
                </span>
                <span>
                  <Icon name="eye" />
                  {viewsLabel(rec.views)}
                </span>
              </div>
            </div>
          </div>
          {rec.description && <p className="watch-desc">{rec.description}</p>}

          {related.length > 0 && (
            <section className="section">
              <div className="section-head">
                <h2>Ещё из этого раздела</h2>
                <Link to={`/archive?category=${rec.category}`} className="link-more">
                  Все <Icon name="chevronRight" />
                </Link>
              </div>
              <div className="grid">
                {related.map((r) => (
                  <RecordingCard key={r.id} rec={r} />
                ))}
              </div>
            </section>
          )}
        </div>
      )}
    </div>
  )
}
