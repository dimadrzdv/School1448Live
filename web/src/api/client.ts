import type {
  Recording,
  RecordingPage,
  ServerStatus,
  StreamInfo,
  StreamInput,
} from './types'

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
  ) {
    super(message)
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body && typeof init.body === 'string' && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }
  const res = await fetch(path, { ...init, headers, credentials: 'same-origin' })
  if (!res.ok) {
    let message = `Ошибка ${res.status}`
    let code: string | undefined
    try {
      const body = await res.json()
      message = body.error ?? message
      code = body.code
    } catch {
      /* not JSON */
    }
    throw new ApiError(message, res.status, code)
  }
  if (res.status === 204 || res.status === 202) return undefined as T
  return res.json() as Promise<T>
}

const json = (body: unknown) => JSON.stringify(body)

export interface RecordingQuery {
  q?: string
  category?: string
  year?: number
  limit?: number
  offset?: number
}

function query(params: object): string {
  const s = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== '') s.set(k, String(v))
  }
  const str = s.toString()
  return str ? `?${str}` : ''
}

export const api = {
  streams: () => request<StreamInfo[]>('/api/streams'),
  stream: (id: string) => request<StreamInfo>(`/api/streams/${encodeURIComponent(id)}`),
  recordings: (q: RecordingQuery = {}) => request<RecordingPage>(`/api/recordings${query(q)}`),
  recording: (id: string) => request<Recording>(`/api/recordings/${encodeURIComponent(id)}`),
  addView: (id: string) =>
    request<void>(`/api/recordings/${encodeURIComponent(id)}/view`, { method: 'POST' }).catch(() => {}),

  admin: {
    login: (code: string) => request<{ ok: true }>('/api/admin/login', { method: 'POST', body: json({ code }) }),
    logout: () => request<void>('/api/admin/logout', { method: 'POST' }),
    me: () => request<{ admin: true }>('/api/admin/me'),
    status: () => request<ServerStatus>('/api/admin/status'),
    streams: () => request<StreamInfo[]>('/api/admin/streams'),
    createStream: (input: StreamInput) =>
      request<StreamInfo>('/api/admin/streams', { method: 'POST', body: json(input) }),
    updateStream: (id: string, input: StreamInput) =>
      request<StreamInfo>(`/api/admin/streams/${id}`, { method: 'PATCH', body: json(input) }),
    deleteStream: (id: string) => request<void>(`/api/admin/streams/${id}`, { method: 'DELETE' }),
    endStream: (id: string) => request<void>(`/api/admin/streams/${id}/end`, { method: 'POST' }),
    uploadPoster: (id: string, jpeg: Blob) =>
      request<void>(`/api/admin/streams/${id}/poster`, {
        method: 'POST',
        body: jpeg,
        headers: { 'Content-Type': 'image/jpeg' },
      }),
    recordings: (q: RecordingQuery = {}) => request<RecordingPage>(`/api/admin/recordings${query(q)}`),
    updateRecording: (id: string, patch: Partial<Pick<Recording, 'title' | 'description' | 'category' | 'hidden'>>) =>
      request<Recording>(`/api/admin/recordings/${id}`, { method: 'PATCH', body: json(patch) }),
    deleteRecording: (id: string) => request<void>(`/api/admin/recordings/${id}`, { method: 'DELETE' }),
  },
}
