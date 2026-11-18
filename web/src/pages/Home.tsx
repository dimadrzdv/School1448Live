import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import { api } from '../api/client'
import type { Recording, StreamInfo } from '../api/types'
import { CategoryChip, Countdown, EventCard, RecordingCard, SkeletonGrid } from '../components/Cards'
import { LivePlayer } from '../components/LivePlayer'
import { CATEGORIES } from '../lib/categories'
import { formatWhen, viewersLabel } from '../lib/format'
import { Icon } from '../lib/icons'
import { useLive, useRefreshOnFocus, useViewerCount } from '../lib/live'

export function Home() {
  const { streams, archiveVersion } = useLive()
  useRefreshOnFocus()
  const [recent, setRecent] = useState<Recording[] | null>(null)

  useEffect(() => {
    api
      .recordings({ limit: 6 })
      .then((p) => setRecent(p.items))
      .catch(() => setRecent([]))
  }, [archiveVersion])

  // Broadcasts with a connected camera first.
  const live = (streams?.filter((s) => s.status === 'live' || s.status === 'reconnecting') ?? []).sort(
    (a, b) => Number(b.status === 'live') - Number(a.status === 'live'),
  )
  const upcoming = streams?.filter((s) => s.status === 'scheduled') ?? []
  const next = upcoming.find((s) => s.scheduledAt && s.scheduledAt > Date.now()) ?? upcoming[0]

  return (
    <div className="container">
      {live.length > 0 ? <LiveHero stream={live[0]} /> : <Hero next={next} loading={streams === null} />}

      {live.length > 1 && (
        <section className="section">
          <div className="section-head">
            <h2>Сейчас в эфире</h2>
          </div>
          <div className="event-grid">
            {live.slice(1).map((s) => (
              <EventCard key={s.id} stream={s} />
            ))}
          </div>
        </section>
      )}

      {upcoming.length > 0 && (
        <section className="section">
          <div className="section-head">
            <div>
              <h2>Скоро в эфире</h2>
              <p>Трансляции начнутся здесь автоматически</p>
            </div>
            <Link to="/schedule" className="link-more">
              Расписание <Icon name="chevronRight" />
            </Link>
          </div>
          <div className="event-grid">
            {upcoming.slice(0, 4).map((s) => (
              <EventCard key={s.id} stream={s} />
            ))}
          </div>
        </section>
      )}

      <section className="section">
        <div className="section-head">
          <div>
            <h2>Свежие записи</h2>
            <p>Пропустили эфир? Всё сохраняется в архиве</p>
          </div>
          <Link to="/archive" className="link-more">
            Весь архив <Icon name="chevronRight" />
          </Link>
        </div>
        {recent === null ? (
          <SkeletonGrid count={3} />
        ) : recent.length === 0 ? (
          <div className="card state-card">
            <div className="state-icon">
              <Icon name="archive" />
            </div>
            <h2>Архив пока пуст</h2>
            <p>После первой трансляции запись появится здесь автоматически.</p>
          </div>
        ) : (
          <div className="grid">
            {recent.map((r) => (
              <RecordingCard key={r.id} rec={r} />
            ))}
          </div>
        )}
      </section>

      <section className="section">
        <div className="section-head">
          <h2>Разделы архива</h2>
        </div>
        <div className="cat-tiles">
          {CATEGORIES.map((c) => (
            <Link key={c.id} to={`/archive?category=${c.id}`} className="card cat-tile" data-cat={c.id}>
              <span className="cat-circle">
                <Icon name={c.icon} />
              </span>
              {c.plural}
            </Link>
          ))}
        </div>
      </section>
    </div>
  )
}

function Hero({ next, loading }: { next?: StreamInfo; loading: boolean }) {
  return (
    <section className="hero">
      <svg className="hero-waves" viewBox="0 0 200 200" aria-hidden>
        {[30, 50, 70, 90].map((r) => (
          <circle key={r} cx="100" cy="100" r={r} fill="none" stroke="#fff" strokeWidth="1.5" />
        ))}
        <circle cx="100" cy="100" r="12" fill="#fff" />
      </svg>
      <div className="hero-inner">
        <div>
          <span className="hero-eyebrow">
            <Icon name="radio" />
            Шуваловская школа № 1448
          </span>
          <h1>
            Школьная жизнь — <em>в прямом эфире</em>
          </h1>
          <p className="hero-lead">
            Концерты, праздники, соревнования и линейки: смотрите вживую с любого устройства, а пропущенное — в
            архиве.
          </p>
          <div className="hero-actions">
            <Link to="/archive" className="btn btn-white">
              <Icon name="play" />
              Смотреть архив
            </Link>
            <Link to="/schedule" className="btn btn-glass">
              <Icon name="calendar" />
              Расписание
            </Link>
          </div>
        </div>
        <div className="next-card">
          {loading ? (
            <div style={{ height: 180 }} />
          ) : next ? (
            <>
              <span className="label">Ближайший эфир</span>
              <h3>{next.title}</h3>
              <p className="when">{next.scheduledAt ? formatWhen(next.scheduledAt) : 'Время уточняется'}</p>
              {next.scheduledAt && next.scheduledAt > Date.now() && <Countdown target={next.scheduledAt} />}
              <div style={{ marginTop: 18, display: 'flex', gap: 10, alignItems: 'center', flexWrap: 'wrap' }}>
                <CategoryChip category={next.category} />
                <Link to={`/live/${next.id}`} className="link-more" style={{ color: '#fff' }}>
                  Страница эфира <Icon name="chevronRight" />
                </Link>
              </div>
            </>
          ) : (
            <>
              <span className="label">Ближайший эфир</span>
              <h3>Пока не запланирован</h3>
              <p className="when">
                Как только администрация назначит трансляцию, она появится здесь — с обратным отсчётом.
              </p>
            </>
          )}
        </div>
      </div>
    </section>
  )
}

function LiveHero({ stream }: { stream: StreamInfo }) {
  const viewers = useViewerCount(stream.id, stream.viewers)
  return (
    <section className="live-hero">
      <LivePlayer stream={stream} />
      <div className="live-hero-info">
        <div>
          <h1>{stream.title}</h1>
          <div className="meta">
            <span className="badge-live">
              <span className="live-dot" />В эфире
            </span>
            <CategoryChip category={stream.category} />
            <span>
              <Icon name="eye" />
              {viewersLabel(viewers)}
            </span>
          </div>
        </div>
        <Link to={`/live/${stream.id}`} className="btn btn-secondary">
          Страница эфира
          <Icon name="chevronRight" />
        </Link>
      </div>
    </section>
  )
}
