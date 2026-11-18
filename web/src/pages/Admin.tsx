import { useCallback, useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router'
import { ApiError, api } from '../api/client'
import type { Category, Recording, ServerStatus, StreamInfo, StreamInput } from '../api/types'
import { CategoryChip, Thumb } from '../components/Cards'
import { CATEGORIES } from '../lib/categories'
import { formatBitrate, formatDate, formatDuration, formatTime, formatWhen, fromLocalInput, toLocalInput, viewersLabel } from '../lib/format'
import { Icon } from '../lib/icons'
import { useLive } from '../lib/live'
import '../styles/admin.css'

export function Admin() {
  const [auth, setAuth] = useState<'unknown' | 'out' | 'in'>('unknown')

  useEffect(() => {
    document.title = 'Администрация · Школа 1448'
    api.admin
      .me()
      .then(() => setAuth('in'))
      .catch(() => setAuth('out'))
  }, [])

  if (auth === 'unknown') return <div className="container section" />
  if (auth === 'out') return <Login onDone={() => setAuth('in')} />
  return <Dashboard onLogout={() => setAuth('out')} />
}

function Login({ onDone }: { onDone: () => void }) {
  const [code, setCode] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    setBusy(true)
    setError(null)
    try {
      await api.admin.login(code)
      onDone()
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Нет связи с сервером')
      setCode('')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="container">
      <form className="card login-card" onSubmit={submit}>
        <div className="state-icon">
          <Icon name="lock" />
        </div>
        <h1>Вход для администрации</h1>
        <p>Введите секретный код, чтобы вести эфиры и управлять архивом.</p>
        <label className="sr-only" htmlFor="code">
          Секретный код
        </label>
        <input
          id="code"
          className="input code-input"
          type="password"
          autoComplete="current-password"
          autoFocus
          placeholder="Секретный код"
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <button className="btn btn-primary" disabled={busy || !code.trim()} style={{ width: '100%' }}>
          {busy ? 'Проверяем…' : 'Войти'}
        </button>
      </form>
    </div>
  )
}

function Dashboard({ onLogout }: { onLogout: () => void }) {
  const [tab, setTab] = useState<'streams' | 'archive'>('streams')
  const logout = async () => {
    await api.admin.logout().catch(() => {})
    onLogout()
  }
  return (
    <div className="container">
      <div className="page-head admin-head">
        <div>
          <h1>Панель администрации</h1>
          <p>Эфиры, студия и архив школы</p>
        </div>
        <button className="btn btn-ghost" onClick={logout}>
          <Icon name="logout" />
          Выйти
        </button>
      </div>
      <div className="tabs" role="tablist">
        <button role="tab" aria-selected={tab === 'streams'} onClick={() => setTab('streams')}>
          <Icon name="radio" />
          Эфиры
        </button>
        <button role="tab" aria-selected={tab === 'archive'} onClick={() => setTab('archive')}>
          <Icon name="archive" />
          Архив
        </button>
      </div>
      {tab === 'streams' ? <StreamsTab /> : <ArchiveTab />}
    </div>
  )
}

// ---------------------------------------------------------------------------

const emptyInput: StreamInput = { title: '', description: '', category: 'other', scheduledAt: null }

function StreamForm({
  initial = emptyInput,
  submitLabel,
  onSubmit,
  onStartNow,
  onCancel,
}: {
  initial?: StreamInput
  submitLabel: string
  onSubmit: (input: StreamInput) => Promise<void>
  onStartNow?: (input: StreamInput) => Promise<void>
  onCancel?: () => void
}) {
  const [title, setTitle] = useState(initial.title)
  const [description, setDescription] = useState(initial.description)
  const [category, setCategory] = useState<Category>(initial.category)
  const [when, setWhen] = useState(initial.scheduledAt ? toLocalInput(initial.scheduledAt) : '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const input = (): StreamInput => ({ title: title.trim(), description, category, scheduledAt: fromLocalInput(when) })

  const run = async (fn: (i: StreamInput) => Promise<void>, needsDate: boolean) => {
    if (!title.trim()) {
      setError('Введите название')
      return
    }
    if (needsDate && !when) {
      setError('Укажите дату и время, чтобы запланировать эфир')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await fn(input())
      if (!onCancel) {
        setTitle('')
        setDescription('')
        setWhen('')
      }
    } catch (e) {
      setError(e instanceof ApiError ? e.message : 'Не удалось сохранить')
    } finally {
      setBusy(false)
    }
  }

  return (
    <form
      className="stream-form"
      onSubmit={(e) => {
        e.preventDefault()
        void run(onSubmit, !onCancel)
      }}
    >
      <div className="field wide">
        <label htmlFor="sf-title">Название</label>
        <input
          id="sf-title"
          className="input"
          maxLength={120}
          placeholder="Например: Новогодний концерт 5-х классов"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
      </div>
      <div className="field">
        <label htmlFor="sf-cat">Категория</label>
        <select id="sf-cat" className="select" value={category} onChange={(e) => setCategory(e.target.value as Category)}>
          {CATEGORIES.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor="sf-when">Дата и время (МСК)</label>
        <input id="sf-when" className="input" type="datetime-local" value={when} onChange={(e) => setWhen(e.target.value)} />
      </div>
      <div className="field wide">
        <label htmlFor="sf-desc">Описание (необязательно)</label>
        <textarea
          id="sf-desc"
          className="textarea"
          maxLength={2000}
          rows={2}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>
      {error && (
        <p className="form-error wide" role="alert">
          {error}
        </p>
      )}
      <div className="form-actions wide">
        {onStartNow && (
          <button type="button" className="btn btn-danger" disabled={busy} onClick={() => void run(onStartNow, false)}>
            <span className="live-dot" />
            Начать эфир сейчас
          </button>
        )}
        <button type="submit" className={onStartNow ? 'btn btn-secondary' : 'btn btn-primary'} disabled={busy}>
          {submitLabel}
        </button>
        {onCancel && (
          <button type="button" className="btn btn-ghost" onClick={onCancel}>
            Отмена
          </button>
        )}
      </div>
    </form>
  )
}

function StreamsTab() {
  const navigate = useNavigate()
  const { streamsVersion, counts } = useLive()
  const [streams, setStreams] = useState<StreamInfo[] | null>(null)
  const [status, setStatus] = useState<ServerStatus | null>(null)
  const [editing, setEditing] = useState<string | null>(null)

  const load = useCallback(() => {
    api.admin.streams().then(setStreams).catch(() => setStreams([]))
    api.admin.status().then(setStatus).catch(() => {})
  }, [])

  useEffect(load, [load, streamsVersion])
  useEffect(() => {
    const t = window.setInterval(() => api.admin.status().then(setStatus).catch(() => {}), 5000)
    return () => window.clearInterval(t)
  }, [])

  const create = async (input: StreamInput) => {
    await api.admin.createStream(input)
    load()
  }
  const startNow = async (input: StreamInput) => {
    const s = await api.admin.createStream({ ...input, scheduledAt: null })
    navigate(`/studio/${s.id}`)
  }
  const end = async (s: StreamInfo) => {
    if (!confirm(`Завершить эфир «${s.title}»? Запись появится в архиве.`)) return
    await api.admin.endStream(s.id)
    setTimeout(load, 500)
  }
  const remove = async (s: StreamInfo) => {
    if (!confirm(`Удалить запланированный эфир «${s.title}»?`)) return
    await api.admin.deleteStream(s.id)
    load()
  }

  const active = streams?.filter((s) => s.status !== 'ended') ?? []
  const past = streams?.filter((s) => s.status === 'ended').slice(0, 10) ?? []

  return (
    <>
      <section className="card admin-card">
        <h2>Новый эфир</h2>
        <StreamForm submitLabel="Запланировать" onSubmit={create} onStartNow={startNow} />
      </section>

      {status && status.rooms.length > 0 && (
        <section className="card admin-card">
          <h2>
            <Icon name="server" /> Сервер
          </h2>
          {status.rooms.map((r) => {
            const share = Math.min(1, r.egressBps / status.egressLimitBps)
            const title = streams?.find((s) => s.id === r.streamId)?.title ?? r.streamId
            return (
              <div key={r.streamId} className="room-stats">
                <div className="room-stats-head">
                  <strong>{title}</strong>
                  <span className={`badge-muted ${r.publisherConnected ? 'badge-success' : 'badge-warning'}`}>
                    {r.publisherConnected ? 'Камера на связи' : 'Ждём камеру'}
                  </span>
                </div>
                <div className="meta">
                  <span>
                    <Icon name="eye" />
                    {viewersLabel(r.viewers)} (в школьной сети: {r.lanViewers})
                  </span>
                  {r.videoCodec && <span>Кодек: {r.videoCodec}</span>}
                  {r.layers.map((l) => (
                    <span key={l.layer}>
                      {['Низкое', 'Среднее', 'Высокое'][l.layer]}: {formatBitrate(l.bitrate)} · {l.viewers}
                    </span>
                  ))}
                </div>
                <div className="meter" title="Загрузка интернет-канала школы">
                  <span style={{ width: `${share * 100}%` }} data-level={share > 0.85 ? 'high' : share > 0.6 ? 'mid' : 'low'} />
                </div>
                <small className="muted">
                  Канал: {formatBitrate(r.egressBps)} из {formatBitrate(status.egressLimitBps)}
                </small>
              </div>
            )
          })}
          {status.pendingJobs > 0 && <p className="muted">Обрабатывается записей: {status.pendingJobs}</p>}
        </section>
      )}

      <section className="section" style={{ marginTop: 32 }}>
        <div className="section-head">
          <h2>Эфиры</h2>
        </div>
        {streams === null ? (
          <div className="skeleton" style={{ height: 120 }} />
        ) : active.length === 0 ? (
          <p className="muted">Нет активных или запланированных эфиров.</p>
        ) : (
          <div className="card admin-list">
            {active.map((s) =>
              editing === s.id ? (
                <div key={s.id} className="admin-row editing">
                  <StreamForm
                    initial={s}
                    submitLabel="Сохранить"
                    onSubmit={async (input) => {
                      await api.admin.updateStream(s.id, input)
                      setEditing(null)
                      load()
                    }}
                    onCancel={() => setEditing(null)}
                  />
                </div>
              ) : (
                <div key={s.id} className="admin-row">
                  <div className="admin-row-main">
                    <div className="meta">
                      {s.status === 'scheduled' ? (
                        <span className="badge-muted">
                          {s.scheduledAt ? formatWhen(s.scheduledAt) : 'Без даты'}
                        </span>
                      ) : s.status === 'live' ? (
                        <span className="badge-live">
                          <span className="live-dot" />В эфире
                        </span>
                      ) : (
                        <span className="badge-muted badge-warning">Переподключение</span>
                      )}
                      <CategoryChip category={s.category} />
                      {s.status !== 'scheduled' && <span>{viewersLabel(counts[s.id] ?? s.viewers)}</span>}
                    </div>
                    <h3>{s.title}</h3>
                  </div>
                  <div className="admin-row-actions">
                    <Link to={`/studio/${s.id}`} className="btn btn-primary btn-sm">
                      <Icon name="video" />
                      Студия
                    </Link>
                    {s.status === 'scheduled' ? (
                      <>
                        <button className="btn btn-secondary btn-sm btn-icon" onClick={() => setEditing(s.id)} aria-label="Изменить">
                          <Icon name="pencil" />
                        </button>
                        <button className="btn btn-secondary btn-sm btn-icon" onClick={() => void remove(s)} aria-label="Удалить">
                          <Icon name="trash" />
                        </button>
                      </>
                    ) : (
                      <button className="btn btn-danger btn-sm" onClick={() => void end(s)}>
                        Завершить
                      </button>
                    )}
                  </div>
                </div>
              ),
            )}
          </div>
        )}

        {past.length > 0 && (
          <>
            <h3 className="muted-head">Недавно завершённые</h3>
            <div className="card admin-list">
              {past.map((s) => (
                <div key={s.id} className="admin-row compact">
                  <div className="admin-row-main">
                    <h3>{s.title}</h3>
                    <div className="meta">
                      <span>{s.startedAt ? `${formatDate(s.startedAt)}, ${formatTime(s.startedAt)}` : ''}</span>
                      {s.startedAt && s.endedAt && <span>{formatDuration(s.endedAt - s.startedAt)}</span>}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </>
        )}
      </section>
    </>
  )
}

// ---------------------------------------------------------------------------

const STATUS_LABEL: Record<Recording['status'], string> = {
  recording: 'Идёт запись',
  processing: 'Обработка…',
  ready: 'Готово',
  failed: 'Ошибка',
}

function ArchiveTab() {
  const { archiveVersion } = useLive()
  const [items, setItems] = useState<Recording[] | null>(null)
  const [total, setTotal] = useState(0)
  const [editing, setEditing] = useState<string | null>(null)

  const load = useCallback((offset = 0) => {
    api.admin
      .recordings({ limit: 30, offset })
      .then((p) => {
        setItems((prev) => (offset && prev ? [...prev, ...p.items] : p.items))
        setTotal(p.total)
      })
      .catch(() => setItems([]))
  }, [])
  useEffect(() => load(), [load, archiveVersion])

  const patch = async (r: Recording, change: Partial<Recording>) => {
    const updated = await api.admin.updateRecording(r.id, change)
    setItems((list) => list?.map((x) => (x.id === r.id ? updated : x)) ?? null)
  }
  const remove = async (r: Recording) => {
    if (!confirm(`Удалить запись «${r.title}» навсегда? Это действие нельзя отменить.`)) return
    await api.admin.deleteRecording(r.id)
    setItems((list) => list?.filter((x) => x.id !== r.id) ?? null)
  }

  if (items === null) return <div className="skeleton" style={{ height: 200, marginTop: 24 }} />
  if (items.length === 0) return <p className="muted" style={{ marginTop: 24 }}>Записей пока нет.</p>

  return (
    <div className="card admin-list" style={{ marginTop: 24 }}>
      {items.map((r) => (
        <div key={r.id} className={`admin-row rec-row${r.hidden ? ' is-hidden' : ''}`}>
          <Link to={r.status === 'ready' ? `/archive/${r.id}` : '#'} className="rec-row-thumb">
            <Thumb src={r.thumbnailUrl} category={r.category} duration={formatDuration(r.durationMs)} />
          </Link>
          {editing === r.id ? (
            <RecordingEditor
              rec={r}
              onSave={async (change) => {
                await patch(r, change)
                setEditing(null)
              }}
              onCancel={() => setEditing(null)}
            />
          ) : (
            <div className="admin-row-main">
              <div className="meta">
                <span className={`badge-muted ${r.status === 'ready' ? 'badge-success' : r.status === 'failed' ? 'badge-warning' : ''}`}>
                  {STATUS_LABEL[r.status]}
                </span>
                {r.hidden && <span className="badge-muted">Скрыта</span>}
                <CategoryChip category={r.category} />
                <span>
                  {formatDate(r.recordedAt)}, {formatTime(r.recordedAt)}
                </span>
                {r.qualities.length > 0 && <span>{r.qualities.map((q) => `${q}p`).join(' / ')}</span>}
              </div>
              <h3>{r.title}</h3>
              {r.error && <p className="form-error">{r.error}</p>}
            </div>
          )}
          {editing !== r.id && (
            <div className="admin-row-actions">
              <button className="btn btn-secondary btn-sm btn-icon" onClick={() => setEditing(r.id)} aria-label="Изменить">
                <Icon name="pencil" />
              </button>
              <button
                className="btn btn-secondary btn-sm btn-icon"
                onClick={() => void patch(r, { hidden: !r.hidden })}
                aria-label={r.hidden ? 'Показать в архиве' : 'Скрыть из архива'}
                title={r.hidden ? 'Показать в архиве' : 'Скрыть из архива'}
              >
                <Icon name={r.hidden ? 'eye' : 'eyeOff'} />
              </button>
              {r.status !== 'recording' && (
                <button className="btn btn-secondary btn-sm btn-icon" onClick={() => void remove(r)} aria-label="Удалить">
                  <Icon name="trash" />
                </button>
              )}
            </div>
          )}
        </div>
      ))}
      {items.length < total && (
        <div className="load-more" style={{ padding: 16, margin: 0 }}>
          <button className="btn btn-secondary" onClick={() => load(items.length)}>
            Показать ещё
          </button>
        </div>
      )}
    </div>
  )
}

function RecordingEditor({
  rec,
  onSave,
  onCancel,
}: {
  rec: Recording
  onSave: (change: Partial<Recording>) => Promise<void>
  onCancel: () => void
}) {
  const [title, setTitle] = useState(rec.title)
  const [description, setDescription] = useState(rec.description)
  const [category, setCategory] = useState<Category>(rec.category)
  const [error, setError] = useState<string | null>(null)
  return (
    <form
      className="stream-form admin-row-main"
      onSubmit={async (e) => {
        e.preventDefault()
        try {
          await onSave({ title, description, category })
        } catch (err) {
          setError(err instanceof ApiError ? err.message : 'Не удалось сохранить')
        }
      }}
    >
      <div className="field wide">
        <label>Название</label>
        <input className="input" value={title} maxLength={120} onChange={(e) => setTitle(e.target.value)} />
      </div>
      <div className="field">
        <label>Категория</label>
        <select className="select" value={category} onChange={(e) => setCategory(e.target.value as Category)}>
          {CATEGORIES.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </select>
      </div>
      <div className="field wide">
        <label>Описание</label>
        <textarea className="textarea" rows={2} value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      {error && <p className="form-error wide">{error}</p>}
      <div className="form-actions wide">
        <button className="btn btn-primary btn-sm">Сохранить</button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={onCancel}>
          Отмена
        </button>
      </div>
    </form>
  )
}
