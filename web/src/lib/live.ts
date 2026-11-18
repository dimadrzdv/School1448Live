import { useEffect, useSyncExternalStore } from 'react'
import { api } from '../api/client'
import type { StreamInfo } from '../api/types'

/** Live/upcoming streams and viewer counts, kept fresh by one shared SSE connection. */
interface LiveState {
  streams: StreamInfo[] | null
  counts: Record<string, number>
  /** Bumped whenever streams change, so pages showing a single stream can refetch. */
  streamsVersion: number
  archiveVersion: number
}

let state: LiveState = { streams: null, counts: {}, streamsVersion: 0, archiveVersion: 0 }
const listeners = new Set<() => void>()
let source: EventSource | null = null
let refreshing: Promise<void> | null = null

function set(patch: Partial<LiveState>) {
  state = { ...state, ...patch }
  listeners.forEach((l) => l())
}

function refresh() {
  refreshing ??= api
    .streams()
    .then((streams) => {
      const counts = { ...state.counts }
      for (const s of streams) counts[s.id] = s.viewers
      set({ streams, counts, streamsVersion: state.streamsVersion + 1 })
    })
    .catch(() => {})
    .finally(() => {
      refreshing = null
    })
}

function connect() {
  if (source || typeof EventSource === 'undefined') return
  source = new EventSource('/api/events')
  source.onopen = refresh
  source.onmessage = (e) => {
    try {
      const msg = JSON.parse(e.data)
      if (msg.type === 'streams') refresh()
      else if (msg.type === 'viewers') set({ counts: { ...state.counts, ...msg.counts } })
      else if (msg.type === 'archive') set({ archiveVersion: state.archiveVersion + 1 })
    } catch {
      /* ignore */
    }
  }
  // EventSource reconnects by itself; refetch when it does.
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  connect()
  if (state.streams === null) refresh()
  return () => {
    listeners.delete(listener)
  }
}

export function useLive(): LiveState {
  return useSyncExternalStore(subscribe, () => state, () => state)
}

export function useViewerCount(id: string | undefined, fallback = 0): number {
  const { counts } = useLive()
  return (id ? counts[id] : undefined) ?? fallback
}

/** Refetches when the tab becomes visible again (phones suspend background tabs). */
export function useRefreshOnFocus() {
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === 'visible') refresh()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => document.removeEventListener('visibilitychange', onVisible)
  }, [])
}
