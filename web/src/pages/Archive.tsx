import { useEffect, useRef, useState } from 'react'
import { useSearchParams } from 'react-router'
import { api } from '../api/client'
import type { Category, Recording } from '../api/types'
import { RecordingCard, SkeletonGrid } from '../components/Cards'
import { CATEGORIES } from '../lib/categories'
import { plural } from '../lib/format'
import { Icon } from '../lib/icons'
import { useLive } from '../lib/live'

const PAGE = 24
// With every tag on screen each group opens with two rows on desktop.
const PREVIEW = 8

type Group = { items: Recording[]; total: number }

export function Archive() {
  const [params, setParams] = useSearchParams()
  // Unknown values (old links to removed tags) show every group.
  const tag = CATEGORIES.find((c) => c.id === params.get('category'))?.id
  const year = params.get('year') ?? ''
  const q = params.get('q') ?? ''
  const [search, setSearch] = useState(q)
  const [groups, setGroups] = useState<Partial<Record<Category, Group>> | null>(null)
  const [years, setYears] = useState<number[]>([])
  const [loadingMore, setLoadingMore] = useState<Category | null>(null)
  const { archiveVersion } = useLive()
  const debounce = useRef<number | undefined>(undefined)
  // Bumped on every reload so late responses for old filters are dropped.
  const generation = useRef(0)

  useEffect(() => {
    document.title = 'Архив · Школа 1448'
  }, [])

  useEffect(() => {
    const gen = ++generation.current
    const tags = tag ? [tag] : CATEGORIES.map((c) => c.id)
    const y = year ? Number(year) : undefined
    Promise.all(tags.map((t) => api.recordings({ q, year: y, category: t, limit: tag ? PAGE : PREVIEW })))
      .then((pages) => {
        if (gen !== generation.current) return
        setGroups(Object.fromEntries(pages.map((p, i) => [tags[i], { items: p.items, total: p.total }])))
        if (pages[0]?.years) setYears(pages[0].years)
      })
      .catch(() => gen === generation.current && setGroups({}))
  }, [q, tag, year, archiveVersion])

  const update = (key: string, value: string) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(next, { replace: true })
  }

  const onSearch = (value: string) => {
    setSearch(value)
    window.clearTimeout(debounce.current)
    debounce.current = window.setTimeout(() => update('q', value.trim()), 300)
  }

  const loadMore = async (t: Category) => {
    const loaded = groups?.[t]?.items.length
    if (loaded === undefined) return
    const gen = generation.current
    setLoadingMore(t)
    try {
      const p = await api.recordings({ q, year: year ? Number(year) : undefined, category: t, limit: PAGE, offset: loaded })
      if (gen !== generation.current) return
      setGroups((prev) => prev && { ...prev, [t]: { items: [...(prev[t]?.items ?? []), ...p.items], total: p.total } })
    } finally {
      setLoadingMore(null)
    }
  }

  const total = groups ? Object.values(groups).reduce((n, g) => n + g.total, 0) : 0
  const visible = groups ? CATEGORIES.filter((c) => (!tag || c.id === tag) && (groups[c.id]?.total ?? 0) > 0) : []

  return (
    <div className="container">
      <div className="page-head">
        <h1>Архив</h1>
        <p>
          {groups === null
            ? 'Загружаем записи…'
            : `${total} ${plural(total, 'запись', 'записи', 'записей')} школьных мероприятий`}
        </p>
      </div>

      <div className="filters">
        <label className="search">
          <Icon name="search" />
          <span className="sr-only">Поиск по названию</span>
          <input
            className="input"
            type="search"
            placeholder="Поиск по названию"
            value={search}
            onChange={(e) => onSearch(e.target.value)}
          />
        </label>
        {years.length > 1 && (
          <select className="select filter-select" value={year} onChange={(e) => update('year', e.target.value)} aria-label="Год">
            <option value="">Все годы</option>
            {years.map((y) => (
              <option key={y} value={y}>
                {y}
              </option>
            ))}
          </select>
        )}
      </div>
      <div className="chips-row" role="group" aria-label="Категория" style={{ marginBottom: 28 }}>
        <button className="filter-chip" aria-pressed={!tag} onClick={() => update('category', '')}>
          Все
        </button>
        {CATEGORIES.map((c) => (
          <button
            key={c.id}
            className="filter-chip"
            data-cat={c.id}
            aria-pressed={tag === c.id}
            onClick={() => update('category', tag === c.id ? '' : c.id)}
          >
            <span className="dot" />
            {c.plural}
          </button>
        ))}
      </div>

      {groups === null ? (
        <SkeletonGrid />
      ) : visible.length === 0 ? (
        <div className="card state-card">
          <div className="state-icon">
            <Icon name="search" />
          </div>
          <h2>Ничего не нашлось</h2>
          <p>Попробуйте изменить запрос или выбрать другую категорию.</p>
        </div>
      ) : (
        visible.map((c) => {
          const g = groups[c.id]!
          return (
            <section key={c.id} className="tag-group" data-cat={c.id} aria-labelledby={`tag-${c.id}`}>
              <div className="section-head">
                <div className="tag-group-title">
                  <span className="cat-circle">
                    <Icon name={c.icon} />
                  </span>
                  <div>
                    <h2 id={`tag-${c.id}`}>{c.plural}</h2>
                    <p>
                      {g.total} {plural(g.total, 'запись', 'записи', 'записей')}
                    </p>
                  </div>
                </div>
              </div>
              <div className="grid">
                {g.items.map((r) => (
                  <RecordingCard key={r.id} rec={r} />
                ))}
              </div>
              {g.items.length < g.total && (
                <div className="load-more">
                  <button className="btn btn-secondary" onClick={() => loadMore(c.id)} disabled={loadingMore === c.id}>
                    {loadingMore === c.id ? 'Загружаем…' : 'Показать ещё'}
                  </button>
                </div>
              )}
            </section>
          )
        })
      )}
    </div>
  )
}
