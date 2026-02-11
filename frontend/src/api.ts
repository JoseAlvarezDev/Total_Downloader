import type {
  AntiBotChallenge,
  DownloadRequest,
  DownloadResult,
  FormatsResponse,
  HistoryEntry,
} from './types'

const API_BASE = import.meta.env.VITE_API_URL ?? 'http://127.0.0.1:8787'

interface ApiError {
  error?: string
  code?: string
  retry_after_seconds?: number
}

export class DownloadLimitError extends Error {
  retryAfterSeconds: number

  constructor(message: string, retryAfterSeconds: number) {
    super(message)
    this.name = 'DownloadLimitError'
    this.retryAfterSeconds = retryAfterSeconds
  }
}

export class BotCheckError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'BotCheckError'
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response
  try {
    response = await fetch(`${API_BASE}${path}`, {
      headers: {
        'Content-Type': 'application/json',
        ...(init?.headers ?? {}),
      },
      ...init,
    })
  } catch {
    throw new Error(`No se pudo conectar al backend (${API_BASE}). Verifica que este ejecutandose.`)
  }

  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as ApiError
    throw new Error(body.error ?? 'No se pudo completar la solicitud.')
  }

  return (await response.json()) as T
}

function decodeFileName(value: string): string {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function extractFilenameFromHeaders(headers: Headers): string | null {
  const contentDisposition = headers.get('content-disposition')
  if (contentDisposition) {
    const utf8Match = contentDisposition.match(/filename\*=UTF-8''([^;]+)/i)
    if (utf8Match?.[1]) {
      return decodeFileName(utf8Match[1].trim())
    }

    const basicMatch = contentDisposition.match(/filename="?([^";]+)"?/i)
    if (basicMatch?.[1]) {
      return basicMatch[1].trim()
    }
  }

  const fallback = headers.get('x-download-filename')
  return fallback?.trim() ? fallback.trim() : null
}

function triggerBrowserDownload(blob: Blob, filename: string): void {
  const objectUrl = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = objectUrl
  link.download = filename
  link.style.display = 'none'
  document.body.appendChild(link)
  link.click()
  link.remove()
  URL.revokeObjectURL(objectUrl)
}

export async function fetchFormats(url: string): Promise<FormatsResponse> {
  return request<FormatsResponse>('/api/formats', {
    method: 'POST',
    body: JSON.stringify({ url }),
  })
}

export async function fetchAntiBotChallenge(): Promise<AntiBotChallenge> {
  return request<AntiBotChallenge>('/api/antibot/challenge')
}

export async function startDownload(payload: DownloadRequest): Promise<DownloadResult> {
  let response: Response
  try {
    response = await fetch(`${API_BASE}/api/download`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(payload),
    })
  } catch {
    throw new Error(`No se pudo conectar al backend (${API_BASE}). Verifica que este ejecutandose.`)
  }

  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as ApiError

    if (body.code === 'DAILY_LIMIT_EXCEEDED') {
      throw new DownloadLimitError(
        body.error ?? 'Has superado el limite diario de descargas.',
        body.retry_after_seconds ?? 24 * 60 * 60,
      )
    }

    if (body.code === 'BOT_CHECK_FAILED') {
      throw new BotCheckError(body.error ?? 'No se pudo validar el filtro anti-bot.')
    }

    throw new Error(body.error ?? 'No se pudo completar la solicitud.')
  }

  const blob = await response.blob()
  const filename = extractFilenameFromHeaders(response.headers) ?? 'total-downloader-file'

  triggerBrowserDownload(blob, filename)

  return { filename }
}

export async function fetchHistory(): Promise<HistoryEntry[]> {
  return request<HistoryEntry[]>('/api/history')
}

export async function clearHistory(): Promise<void> {
  await request<{ status: string }>('/api/history', {
    method: 'DELETE',
  })
}
