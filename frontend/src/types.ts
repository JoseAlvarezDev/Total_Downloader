export type DownloadMode = 'video' | 'audio'
export type DownloadStatus = 'success' | 'failed'

export interface FormatOption {
  format_id: string
  label: string
  resolution: string | null
  ext: string
  has_audio: boolean
}

export interface FormatsResponse {
  title: string
  thumbnail: string | null
  video_options: FormatOption[]
  audio_options: FormatOption[]
}

export interface HistoryEntry {
  id: string
  created_at: string
  url: string
  title: string | null
  thumbnail: string | null
  mode: DownloadMode
  format: string
  status: DownloadStatus
  saved_path: string | null
  error: string | null
}

export interface DownloadRequest {
  url: string
  title?: string
  thumbnail?: string
  mode: DownloadMode
  format_id?: string
  format_label?: string
  has_audio?: boolean
  antibot_challenge_id?: string
  antibot_solution?: number
  antibot_honey?: string
  antibot_elapsed_ms?: number
  turnstile_token?: string
}

export interface DownloadResult {
  filename: string | null
}

export interface AntiBotChallenge {
  challenge_id: string
  nonce: string
  difficulty: number
  expires_in_seconds: number
}
