import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import type { Recording, StreamInfo } from '../api/types'
import { categoryOf } from '../lib/categories'
import { dateTile, formatDate, formatDuration, formatTime, formatWhen, viewsLabel } from '../lib/format'
import { Icon } from '../lib/icons'

export function CategoryChip({ category }: { category: string }) {
  const c = categoryOf(category)
  return (
    <span className="chip" data-cat={c.id}>
      <span className="dot" />
      {c.label}
    </span>
  )
}

export function Thumb({
  src,
  category,
  duration,
}: {
  src: string | null
  category: string
  duration?: string
}) {
  const [failed, setFailed] = useState(false)
  const c = categoryOf(category)
  return (
    <div className="thumb" data-cat={c.id}>
      {src && !failed ? (
        <img src={src} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />
      ) : (
        <div className="thumb-placeholder">
          <Icon name={c.icon} />
        </div>
      )}
      {duration && <span className="duration">{duration}</span>}
      <div className="play">
        <span>
          <Icon name="play" />
        </span>
      </div>
    </div>
  )
}

export function RecordingCard({ rec }: { rec: Recording }) {
  return (
    <Link to={`/archive/${rec.id}`} className="rec-card">
      <Thumb src={rec.thumbnailUrl} category={rec.category} duration={formatDuration(rec.durationMs)} />
      <div className="rec-card-body">
        <div className="meta">
          <CategoryChip category={rec.category} />
          <span>{formatDate(rec.recordedAt)}</span>
        </div>
        <h3>{rec.title}</h3>
        <div className="meta">
          <span>
            <Icon name="eye" />
            {viewsLabel(rec.views)}
          </span>
        </div>
      </div>
    </Link>
  )
}

export function EventCard({ stream }: { stream: StreamInfo }) {
  const when = stream.scheduledAt ?? stream.startedAt ?? Date.now()
  const tile = dateTile(when)
  const c = categoryOf(stream.category)
  const live = stream.status === 'live' || stream.status === 'reconnecting'
  return (
    <Link to={`/live/${stream.id}`} className="card event-card" data-cat={c.id}>
      <div className="date-tile">
        <b>{tile.day}</b>
        <small>{tile.month}</small>
      </div>
      <div style={{ minWidth: 0 }}>
        <h3>{stream.title}</h3>
        <div className="meta">
          {live ? (
            <span className="badge-live">
              <span className="live-dot" />В эфире
            </span>
          ) : (
            <span>
              <Icon name="clock" />
              {stream.scheduledAt ? formatTime(stream.scheduledAt) : 'Время уточняется'}
            </span>
          )}
          <CategoryChip category={stream.category} />
        </div>
      </div>
    </Link>
  )
}

export function useCountdown(target: number | null) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!target) return
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(t)
  }, [target])
  if (!target) return null
  const left = Math.max(0, target - now)
  const s = Math.floor(left / 1000)
  return { days: Math.floor(s / 86400), hours: Math.floor((s % 86400) / 3600), minutes: Math.floor((s % 3600) / 60), seconds: s % 60, done: left === 0 }
}

export function Countdown({ target }: { target: number }) {
  const c = useCountdown(target)
  if (!c) return null
  const pad = (n: number) => String(n).padStart(2, '0')
  return (
    <div className="countdown" role="timer" aria-label={`До начала: ${formatWhen(target)}`}>
      <div>
        <b>{c.days}</b>
        <small>дн</small>
      </div>
      <div>
        <b>{pad(c.hours)}</b>
        <small>час</small>
      </div>
      <div>
        <b>{pad(c.minutes)}</b>
        <small>мин</small>
      </div>
      <div>
        <b>{pad(c.seconds)}</b>
        <small>сек</small>
      </div>
    </div>
  )
}

export function SkeletonGrid({ count = 6 }: { count?: number }) {
  return (
    <div className="grid" aria-hidden>
      {Array.from({ length: count }, (_, i) => (
        <div key={i}>
          <div className="skeleton" style={{ aspectRatio: '16 / 9' }} />
          <div className="skeleton" style={{ height: 18, marginTop: 14, width: '40%' }} />
          <div className="skeleton" style={{ height: 20, marginTop: 10, width: '85%' }} />
        </div>
      ))}
    </div>
  )
}

export function StateCard({
  icon,
  title,
  text,
  children,
}: {
  icon: Parameters<typeof Icon>[0]['name']
  title: string
  text?: string
  children?: React.ReactNode
}) {
  return (
    <div className="card state-card">
      <div className="state-icon">
        <Icon name={icon} />
      </div>
      <h2>{title}</h2>
      {text && <p>{text}</p>}
      {children}
    </div>
  )
}
