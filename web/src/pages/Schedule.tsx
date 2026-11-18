import { useEffect } from 'react'
import { Link } from 'react-router'
import type { StreamInfo } from '../api/types'
import { CategoryChip, StateCard } from '../components/Cards'
import { dayKey, formatTime, relativeDay } from '../lib/format'
import { Icon } from '../lib/icons'
import { useLive, useRefreshOnFocus } from '../lib/live'

export function Schedule() {
  const { streams } = useLive()
  useRefreshOnFocus()

  useEffect(() => {
    document.title = 'Расписание эфиров · Школа 1448'
  }, [])

  const list = streams ?? []
  const groups = new Map<string, { label: string; items: StreamInfo[] }>()
  for (const s of list) {
    const when = s.status === 'scheduled' ? (s.scheduledAt ?? s.startedAt ?? Date.now()) : (s.startedAt ?? Date.now())
    const key = s.status === 'scheduled' && !s.scheduledAt ? 'tbd' : dayKey(when)
    const label = key === 'tbd' ? 'Дата уточняется' : relativeDay(when)
    if (!groups.has(key)) groups.set(key, { label, items: [] })
    groups.get(key)!.items.push(s)
  }
  const ordered = [...groups.entries()].sort(([a], [b]) => (a === 'tbd' ? 1 : b === 'tbd' ? -1 : a.localeCompare(b)))

  return (
    <div className="container">
      <div className="page-head">
        <h1>Расписание</h1>
        <p>Ближайшие трансляции. Откройте эфир заранее — видео запустится само.</p>
      </div>

      {streams === null ? (
        <div className="skeleton" style={{ height: 240, marginTop: 32 }} />
      ) : list.length === 0 ? (
        <div className="section">
          <StateCard icon="calendar" title="Пока ничего не запланировано" text="Загляните в архив — там все прошедшие мероприятия.">
            <Link to="/archive" className="btn btn-primary">
              Открыть архив
            </Link>
          </StateCard>
        </div>
      ) : (
        ordered.map(([key, g]) => (
          <section key={key} className="day-group">
            <h2>{g.label}</h2>
            <div className="card">
              {g.items.map((s) => {
                const live = s.status === 'live' || s.status === 'reconnecting'
                return (
                  <Link key={s.id} to={`/live/${s.id}`} className="schedule-row">
                    <span className="time">
                      {live ? (
                        <span className="badge-live">
                          <span className="live-dot" />
                          Live
                        </span>
                      ) : s.scheduledAt ? (
                        formatTime(s.scheduledAt)
                      ) : (
                        '—'
                      )}
                    </span>
                    <div style={{ minWidth: 0 }}>
                      <h3>{s.title}</h3>
                      {s.description && <p>{s.description}</p>}
                    </div>
                    <div className="meta">
                      <CategoryChip category={s.category} />
                      <Icon name="chevronRight" />
                    </div>
                  </Link>
                )
              })}
            </div>
          </section>
        ))
      )}
    </div>
  )
}
