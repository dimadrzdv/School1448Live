const TZ = 'Europe/Moscow'

const dayMonth = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', timeZone: TZ })
const dayMonthYear = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', year: 'numeric', timeZone: TZ })
const time = new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit', timeZone: TZ })
const weekday = new Intl.DateTimeFormat('ru-RU', { weekday: 'long', day: 'numeric', month: 'long', timeZone: TZ })
const monthShort = new Intl.DateTimeFormat('ru-RU', { month: 'short', timeZone: TZ })
const dayNum = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', timeZone: TZ })
const dayKeyFmt = new Intl.DateTimeFormat('en-CA', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone: TZ })

export const formatTime = (ms: number) => time.format(ms)

export function formatDate(ms: number): string {
  const sameYear = dayKeyFmt.format(ms).slice(0, 4) === dayKeyFmt.format(Date.now()).slice(0, 4)
  return (sameYear ? dayMonth : dayMonthYear).format(ms)
}

export const dayKey = (ms: number) => dayKeyFmt.format(ms)

export function relativeDay(ms: number): string {
  const key = dayKey(ms)
  const today = dayKey(Date.now())
  const tomorrow = dayKey(Date.now() + 86_400_000)
  if (key === today) return 'Сегодня'
  if (key === tomorrow) return 'Завтра'
  const s = weekday.format(ms)
  return s.charAt(0).toUpperCase() + s.slice(1)
}

export function formatWhen(ms: number): string {
  const rel = relativeDay(ms)
  const day = rel === 'Сегодня' || rel === 'Завтра' ? rel.toLowerCase() : formatDate(ms)
  return `${day} в ${formatTime(ms)}`
}

export const dateTile = (ms: number) => ({
  day: dayNum.format(ms),
  month: monthShort.format(ms).replace('.', ''),
})

export function formatDuration(ms: number | null | undefined): string {
  if (!ms || ms < 0) return '—'
  const total = Math.round(ms / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const pad = (n: number) => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`
}

export function plural(n: number, one: string, few: string, many: string): string {
  const mod10 = n % 10
  const mod100 = n % 100
  if (mod10 === 1 && mod100 !== 11) return one
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 10 || mod100 >= 20)) return few
  return many
}

export const viewersLabel = (n: number) => `${n} ${plural(n, 'зритель', 'зрителя', 'зрителей')}`
export const viewsLabel = (n: number) => `${n} ${plural(n, 'просмотр', 'просмотра', 'просмотров')}`

export function formatBitrate(bps: number): string {
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Мбит/с`
  return `${Math.round(bps / 1000)} кбит/с`
}

/** `datetime-local` value in Moscow time <-> epoch ms. */
export function toLocalInput(ms: number): string {
  const d = new Date(ms + 3 * 3600_000)
  return d.toISOString().slice(0, 16)
}
export function fromLocalInput(value: string): number | null {
  if (!value) return null
  const ms = Date.parse(`${value}:00+03:00`)
  return Number.isNaN(ms) ? null : ms
}
