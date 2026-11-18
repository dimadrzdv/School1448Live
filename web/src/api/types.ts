export type Category = 'flag' | 'concert' | 'other'

export type StreamStatus = 'scheduled' | 'live' | 'reconnecting' | 'ended'

export interface StreamInfo {
  id: string
  title: string
  description: string
  category: Category
  status: StreamStatus
  scheduledAt: number | null
  startedAt: number | null
  endedAt: number | null
  viewers: number
  posterUrl: string | null
  recordingId: string | null
}

export type RecordingStatus = 'recording' | 'processing' | 'ready' | 'failed'

export interface Recording {
  id: string
  streamId: string | null
  title: string
  description: string
  category: Category
  status: RecordingStatus
  hidden: boolean
  recordedAt: number
  durationMs: number | null
  width: number | null
  height: number | null
  thumbnailUrl: string | null
  playlistUrl: string | null
  qualities: number[]
  views: number
  error: string | null
}

export interface RecordingPage {
  items: Recording[]
  total: number
  years?: number[]
}

export interface StreamInput {
  title: string
  description: string
  category: Category
  scheduledAt: number | null
}

export interface RoomStats {
  streamId: string
  publisherConnected: boolean
  viewers: number
  lanViewers: number
  egressBps: number
  layers: { layer: number; bitrate: number; viewers: number }[]
  videoCodec: string | null
}

export interface ServerStatus {
  rooms: RoomStats[]
  pendingJobs: number
  egressLimitBps: number
}

export type LayerPref = 'auto' | 'low' | 'mid' | 'high'
