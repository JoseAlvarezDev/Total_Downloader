import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import './App.css'
import {
  BotCheckError,
  clearHistory,
  DownloadLimitError,
  fetchAntiBotChallenge,
  fetchFormats,
  fetchHistory,
  startDownload,
} from './api'
import type {
  AntiBotChallenge,
  DownloadMode,
  FormatOption,
  FormatsResponse,
  HistoryEntry,
} from './types'

const MODE_LABELS: Record<DownloadMode, string> = {
  video: 'Descargar video',
  audio: 'Descargar audio',
}

const MENU_GROUPS = [
  {
    id: 'menu',
    label: 'Menu',
    links: [
      { label: 'Privacidad', href: '#privacidad' },
      { label: 'Terms', href: '#terms' },
    ],
  },
] as const

function formatDate(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    return value
  }

  return new Intl.DateTimeFormat('es-ES', {
    dateStyle: 'short',
    timeStyle: 'short',
  }).format(date)
}

function formatCountdown(seconds: number): string {
  const safeSeconds = Math.max(0, seconds)
  const hours = Math.floor(safeSeconds / 3600)
  const minutes = Math.floor((safeSeconds % 3600) / 60)
  const secs = safeSeconds % 60

  return [hours, minutes, secs].map((value) => value.toString().padStart(2, '0')).join(':')
}

interface BeforeInstallPromptEvent extends Event {
  prompt: () => Promise<void>
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed'; platform: string }>
}

interface TurnstileRenderOptions {
  sitekey: string
  theme?: 'light' | 'dark' | 'auto'
  callback?: (token: string) => void
  'expired-callback'?: () => void
  'error-callback'?: () => void
}

interface TurnstileApi {
  render: (container: string | HTMLElement, options: TurnstileRenderOptions) => string
  reset: (widgetId?: string) => void
  remove: (widgetId: string) => void
}

declare global {
  interface Window {
    turnstile?: TurnstileApi
  }
}

const TURNSTILE_SCRIPT_ID = 'cloudflare-turnstile-script'
const TURNSTILE_CONTAINER_ID = 'turnstile-widget-container'

async function sha256Hex(value: string): Promise<string> {
  const input = new TextEncoder().encode(value)
  const digest = await window.crypto.subtle.digest('SHA-256', input)
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}

async function solveAntiBotChallenge(challenge: AntiBotChallenge): Promise<number> {
  const prefix = '0'.repeat(Math.max(1, challenge.difficulty))
  let attempt = 0

  while (attempt < Number.MAX_SAFE_INTEGER) {
    const hash = await sha256Hex(`${challenge.challenge_id}:${challenge.nonce}:${attempt}`)
    if (hash.startsWith(prefix)) {
      return attempt
    }

    attempt += 1
    if (attempt % 150 === 0) {
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, 0)
      })
    }
  }

  throw new Error('No se encontro solucion anti-bot.')
}

function App() {
  const turnstileSiteKey = (import.meta.env.VITE_TURNSTILE_SITE_KEY ?? '').trim()
  const useTurnstile = turnstileSiteKey.length > 0
  const turnstileWidgetIdRef = useRef<string | null>(null)
  const logoSrc = `${import.meta.env.BASE_URL}image.png`
  const [showSplash, setShowSplash] = useState(true)
  const [isMenuOpen, setIsMenuOpen] = useState(false)
  const [openSubmenuId, setOpenSubmenuId] = useState<string | null>(null)
  const [url, setUrl] = useState('')
  const [mode, setMode] = useState<DownloadMode>('video')
  const [formats, setFormats] = useState<FormatsResponse | null>(null)
  const [selectedFormatId, setSelectedFormatId] = useState('')
  const [selectedFormatLabel, setSelectedFormatLabel] = useState('')
  const [selectedFormatHasAudio, setSelectedFormatHasAudio] = useState(false)
  const [history, setHistory] = useState<HistoryEntry[]>([])
  const [isLoadingFormats, setIsLoadingFormats] = useState(false)
  const [isDownloading, setIsDownloading] = useState(false)
  const [isClearingHistory, setIsClearingHistory] = useState(false)
  const [isPreparingAntiBot, setIsPreparingAntiBot] = useState(false)
  const [antiBotChallenge, setAntiBotChallenge] = useState<AntiBotChallenge | null>(null)
  const [antiBotSolution, setAntiBotSolution] = useState<number | null>(null)
  const [antiBotReadyAt, setAntiBotReadyAt] = useState<number | null>(null)
  const [turnstileToken, setTurnstileToken] = useState('')
  const [antiBotHoneyField, setAntiBotHoneyField] = useState('')
  const [limitRemainingSeconds, setLimitRemainingSeconds] = useState<number | null>(null)
  const [installPromptEvent, setInstallPromptEvent] = useState<BeforeInstallPromptEvent | null>(
    null,
  )
  const [isPwaInstalled, setIsPwaInstalled] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const currentYear = new Date().getFullYear()
  const isLimitBlocked = limitRemainingSeconds !== null && limitRemainingSeconds > 0
  const isAntiBotReady = useTurnstile
    ? Boolean(turnstileToken)
    : antiBotChallenge !== null && antiBotSolution !== null && antiBotReadyAt !== null
  const antiBotStatusLabel = useTurnstile
    ? isAntiBotReady
      ? 'Turnstile validado'
      : 'Completa Turnstile para habilitar descargas.'
    : isAntiBotReady
      ? 'Filtro anti-bot listo'
      : 'Preparando verificacion anti-bot...'

  const options = useMemo(
    () => (mode === 'video' ? formats?.video_options ?? [] : formats?.audio_options ?? []),
    [formats, mode],
  )

  const refreshHistory = useCallback(async () => {
    try {
      const entries = await fetchHistory()
      setHistory(entries)
    } catch {
      // No interrumpimos el flujo principal si falla el historial.
    }
  }, [])

  const prepareAntiBot = useCallback(async () => {
    if (useTurnstile) {
      setIsPreparingAntiBot(false)
      setAntiBotChallenge(null)
      setAntiBotSolution(null)
      setAntiBotReadyAt(null)
      return
    }

    setIsPreparingAntiBot(true)
    try {
      const challenge = await fetchAntiBotChallenge()
      const solvedValue = await solveAntiBotChallenge(challenge)
      setAntiBotChallenge(challenge)
      setAntiBotSolution(solvedValue)
      setAntiBotReadyAt(Date.now())
    } catch (requestError) {
      setAntiBotChallenge(null)
      setAntiBotSolution(null)
      setAntiBotReadyAt(null)
      setError(
        requestError instanceof Error
          ? requestError.message
          : 'No se pudo validar anti-bot por un problema de conexion.',
      )
    } finally {
      setIsPreparingAntiBot(false)
    }
  }, [useTurnstile])

  useEffect(() => {
    void refreshHistory()
  }, [refreshHistory])

  useEffect(() => {
    void prepareAntiBot()
  }, [prepareAntiBot])

  useEffect(() => {
    if (!useTurnstile) {
      setTurnstileToken('')
      return
    }

    let active = true

    const renderTurnstileWidget = () => {
      if (!active || !window.turnstile) {
        return
      }

      const container = document.getElementById(TURNSTILE_CONTAINER_ID)
      if (!container) {
        return
      }

      if (turnstileWidgetIdRef.current) {
        window.turnstile.remove(turnstileWidgetIdRef.current)
        turnstileWidgetIdRef.current = null
      }

      container.replaceChildren()
      turnstileWidgetIdRef.current = window.turnstile.render(container, {
        sitekey: turnstileSiteKey,
        theme: 'dark',
        callback: (token: string) => {
          setTurnstileToken(token)
          setError('')
        },
        'expired-callback': () => {
          setTurnstileToken('')
        },
        'error-callback': () => {
          setTurnstileToken('')
          setError('No se pudo validar Turnstile. Recarga la pagina y reintenta.')
        },
      })
    }

    const onScriptLoad = () => {
      renderTurnstileWidget()
    }
    const onScriptError = () => {
      if (!active) {
        return
      }
      setError('No se pudo cargar el widget de Turnstile.')
    }

    const existingScript = document.getElementById(TURNSTILE_SCRIPT_ID) as HTMLScriptElement | null
    if (window.turnstile) {
      renderTurnstileWidget()
    } else if (existingScript) {
      existingScript.addEventListener('load', onScriptLoad)
      existingScript.addEventListener('error', onScriptError)
    } else {
      const script = document.createElement('script')
      script.id = TURNSTILE_SCRIPT_ID
      script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit'
      script.async = true
      script.defer = true
      script.addEventListener('load', onScriptLoad)
      script.addEventListener('error', onScriptError)
      document.head.appendChild(script)
    }

    return () => {
      active = false
      const script = document.getElementById(TURNSTILE_SCRIPT_ID) as HTMLScriptElement | null
      script?.removeEventListener('load', onScriptLoad)
      script?.removeEventListener('error', onScriptError)

      if (turnstileWidgetIdRef.current && window.turnstile) {
        window.turnstile.remove(turnstileWidgetIdRef.current)
      }
      turnstileWidgetIdRef.current = null
    }
  }, [turnstileSiteKey, useTurnstile])

  useEffect(() => {
    const navigatorWithStandalone = window.navigator as Navigator & { standalone?: boolean }
    const standaloneMode =
      window.matchMedia('(display-mode: standalone)').matches ||
      navigatorWithStandalone.standalone === true
    if (standaloneMode) {
      setIsPwaInstalled(true)
    }

    const handleBeforeInstallPrompt = (event: Event) => {
      event.preventDefault()
      setInstallPromptEvent(event as BeforeInstallPromptEvent)
    }

    const handleAppInstalled = () => {
      setIsPwaInstalled(true)
      setInstallPromptEvent(null)
      setNotice('App instalada correctamente en tu dispositivo.')
    }

    window.addEventListener('beforeinstallprompt', handleBeforeInstallPrompt)
    window.addEventListener('appinstalled', handleAppInstalled)

    return () => {
      window.removeEventListener('beforeinstallprompt', handleBeforeInstallPrompt)
      window.removeEventListener('appinstalled', handleAppInstalled)
    }
  }, [])

  useEffect(() => {
    const timerId = window.setTimeout(() => {
      setShowSplash(false)
    }, 1450)

    return () => {
      window.clearTimeout(timerId)
    }
  }, [])

  useEffect(() => {
    if (limitRemainingSeconds === null || limitRemainingSeconds <= 0) {
      return
    }

    const intervalId = window.setInterval(() => {
      setLimitRemainingSeconds((currentValue) => {
        if (currentValue === null) {
          return null
        }

        return Math.max(0, currentValue - 1)
      })
    }, 1000)

    return () => {
      window.clearInterval(intervalId)
    }
  }, [limitRemainingSeconds])

  useEffect(() => {
    if (options.length === 0) {
      setSelectedFormatId('')
      setSelectedFormatLabel('')
      setSelectedFormatHasAudio(false)
      return
    }

    const isCurrentValid = options.some((item) => item.format_id === selectedFormatId)
    if (!isCurrentValid) {
      setSelectedFormatId(options[0].format_id)
      setSelectedFormatLabel(options[0].label)
      setSelectedFormatHasAudio(options[0].has_audio)
    }
  }, [options, selectedFormatId])

  const handleFormatChange = (event: React.ChangeEvent<HTMLSelectElement>) => {
    const nextId = event.target.value
    const picked = options.find((item) => item.format_id === nextId)

    setSelectedFormatId(nextId)
    setSelectedFormatLabel(picked?.label ?? '')
    setSelectedFormatHasAudio(picked?.has_audio ?? false)
  }

  const handleLoadFormats = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const cleanUrl = url.trim()

    if (!cleanUrl) {
      setError('Pega una URL antes de consultar formatos.')
      setNotice('')
      return
    }

    setIsLoadingFormats(true)
    setError('')
    setNotice('')

    try {
      const payload = await fetchFormats(cleanUrl)
      setFormats(payload)
      setNotice(`Opciones cargadas para: ${payload.title}`)
    } catch (requestError) {
      setFormats(null)
      setError(
        requestError instanceof Error
          ? requestError.message
          : 'No se pudieron cargar los formatos.',
      )
    } finally {
      setIsLoadingFormats(false)
    }
  }

  const handleDownload = async () => {
    const cleanUrl = url.trim()
    if (!cleanUrl) {
      setError('Ingresa una URL valida antes de descargar.')
      return
    }

    if (options.length === 0) {
      setError('Primero carga las opciones para esa URL.')
      return
    }

    if (isPreparingAntiBot || !isAntiBotReady) {
      setError(
        useTurnstile
          ? 'Completa Turnstile para continuar con la descarga.'
          : 'Verificando filtro anti-bot. Espera unos segundos e intenta de nuevo.',
      )
      if (!useTurnstile && !isPreparingAntiBot) {
        void prepareAntiBot()
      }
      return
    }

    setIsDownloading(true)
    setError('')
    setNotice('')

    try {
      const elapsedMs = Math.max(0, Date.now() - (antiBotReadyAt ?? Date.now()))
      const result = await startDownload({
        url: cleanUrl,
        title: formats?.title ?? undefined,
        thumbnail: formats?.thumbnail ?? undefined,
        mode,
        format_id: selectedFormatId,
        format_label: selectedFormatLabel,
        has_audio: selectedFormatHasAudio,
        antibot_challenge_id: antiBotChallenge?.challenge_id,
        antibot_solution: antiBotSolution ?? undefined,
        antibot_honey: antiBotHoneyField,
        antibot_elapsed_ms: elapsedMs,
        turnstile_token: useTurnstile ? turnstileToken : undefined,
      })

      const fileLine = result.filename ? `\nArchivo: ${result.filename}` : ''
      setNotice(`Descarga completada en tu dispositivo.${fileLine}`)
      await refreshHistory()
    } catch (requestError) {
      if (requestError instanceof DownloadLimitError) {
        setLimitRemainingSeconds(requestError.retryAfterSeconds)
        setError('')
        setNotice('')
        await refreshHistory()
        return
      }

      if (requestError instanceof BotCheckError) {
        setError(
          useTurnstile
            ? 'Turnstile rechazo la verificacion. Recarga la pagina y vuelve a intentarlo.'
            : 'Se activo el filtro anti-bot. Verifica la pagina y vuelve a intentarlo en unos segundos.',
        )
        setNotice('')
        await refreshHistory()
        return
      }

      setError(
        requestError instanceof Error
          ? requestError.message
          : 'No fue posible completar la descarga.',
      )
      await refreshHistory()
    } finally {
      setIsDownloading(false)
      if (useTurnstile) {
        const widgetId = turnstileWidgetIdRef.current
        if (widgetId && window.turnstile) {
          window.turnstile.reset(widgetId)
        }
        setTurnstileToken('')
      } else {
        void prepareAntiBot()
      }
    }
  }

  const closeLimitModal = () => {
    setLimitRemainingSeconds(null)
  }

  const toggleSubmenu = (submenuId: string) => {
    setOpenSubmenuId((currentId) => (currentId === submenuId ? null : submenuId))
  }

  const closeMobileMenu = () => {
    setIsMenuOpen(false)
    setOpenSubmenuId(null)
  }

  const handleInstallPwa = async () => {
    if (isPwaInstalled) {
      setNotice('La app ya esta instalada en este dispositivo.')
      setError('')
      return
    }

    if (!installPromptEvent) {
      setError(
        'Tu navegador no permite instalacion directa desde este boton. Usa "Agregar a pantalla de inicio".',
      )
      return
    }

    setError('')
    await installPromptEvent.prompt()
    const choice = await installPromptEvent.userChoice
    if (choice.outcome === 'accepted') {
      setNotice('Instalacion iniciada. Revisa tu dispositivo.')
    } else {
      setNotice('Instalacion cancelada. Puedes intentarlo de nuevo cuando quieras.')
    }

    setInstallPromptEvent(null)
  }

  const handleClearHistory = async () => {
    if (history.length === 0 || isClearingHistory) {
      return
    }

    const shouldClear = window.confirm(
      'Vas a borrar todo el historial reciente. Esta accion no se puede deshacer. Continuar?',
    )
    if (!shouldClear) {
      return
    }

    setIsClearingHistory(true)
    setError('')
    try {
      await clearHistory()
      setHistory([])
      setNotice('Historial eliminado correctamente.')
    } catch (requestError) {
      setError(
        requestError instanceof Error
          ? requestError.message
          : 'No se pudo borrar el historial.',
      )
    } finally {
      setIsClearingHistory(false)
    }
  }

  if (showSplash) {
    return (
      <div className="splash-screen" role="status" aria-live="polite">
        <div className="splash-content">
          <img src={logoSrc} alt="Total Downloader" className="splash-logo" />
          <p className="splash-title">Total Downloader</p>
          <p className="splash-subtitle">Preparando tu centro de descargas...</p>
        </div>
      </div>
    )
  }

  return (
    <div className="app-shell">
      <header className="site-header">
        <a href="#inicio" className="site-brand" onClick={closeMobileMenu}>
          <img src={logoSrc} alt="Total Downloader" className="site-brand-logo" />
          <div className="site-brand-copy">
            <p className="kicker">TOTAL DOWNLOADER</p>
            <strong>Centro de Descargas</strong>
          </div>
        </a>

        <button
          type="button"
          className="menu-toggle"
          aria-label={isMenuOpen ? 'Cerrar menu principal' : 'Abrir menu principal'}
          aria-expanded={isMenuOpen}
          onClick={() => setIsMenuOpen((status) => !status)}
        >
          {isMenuOpen ? 'Cerrar' : 'Menu'}
        </button>

        <nav className={`main-nav ${isMenuOpen ? 'is-open' : ''}`} aria-label="Menu principal">
          <ul className="menu-root">
            {MENU_GROUPS.map((group) => {
              const isOpen = openSubmenuId === group.id

              return (
                <li
                  key={group.id}
                  className={`menu-item ${isOpen ? 'open' : ''}`}
                  onMouseEnter={() => setOpenSubmenuId(group.id)}
                  onMouseLeave={() =>
                    setOpenSubmenuId((currentId) =>
                      currentId === group.id ? null : currentId,
                    )
                  }
                >
                  <button
                    type="button"
                    className="menu-link"
                    aria-expanded={isOpen}
                    onClick={() => toggleSubmenu(group.id)}
                  >
                    {group.label}
                    <span className="menu-caret" aria-hidden="true">
                      ▾
                    </span>
                  </button>

                  <ul className="submenu">
                    {group.links.map((link) => (
                      <li key={link.label}>
                        <a href={link.href} onClick={closeMobileMenu}>
                          {link.label}
                        </a>
                      </li>
                    ))}
                  </ul>
                </li>
              )
            })}
          </ul>
        </nav>
      </header>

      <section className="hero" id="inicio">
        <div className="hero-intro">
          <h1>Descargas de video sin fricciones</h1>
          <p className="hero-copy">
            X, Facebook, TikTok, YouTube, Instagram y mas. Elige formato, resolucion y
            extrae audio en segundos.
          </p>
          <div className="hero-actions">
            <button
              type="button"
              className="pwa-install-button"
              onClick={handleInstallPwa}
              disabled={isPwaInstalled}
            >
              {isPwaInstalled ? 'App instalada' : 'Descargar app (PWA)'}
            </button>
            <a href="#descargar" className="hero-secondary-link">
              Ir al descargador
            </a>
          </div>
        </div>

        <ul className="hero-points">
          <li>Video y audio desde una sola URL.</li>
          <li>Resoluciones ordenadas de mejor a peor.</li>
          <li>Historial con miniatura y titulo.</li>
          <li>Instalable como app PWA.</li>
        </ul>
      </section>

      <main className="main-grid">
        <section className="panel" id="descargar">
          <form className="download-form" onSubmit={handleLoadFormats}>
            <label htmlFor="video-url">URL del video</label>
            <div className="input-row">
              <input
                id="video-url"
                type="url"
                placeholder="https://..."
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                required
              />
              <button type="submit" disabled={isLoadingFormats}>
                {isLoadingFormats ? 'Cargando...' : 'Buscar formatos'}
              </button>
            </div>

            <div className="honey-field" aria-hidden="true">
              <label htmlFor="website-field">Website</label>
              <input
                id="website-field"
                type="text"
                tabIndex={-1}
                autoComplete="off"
                value={antiBotHoneyField}
                onChange={(event) => setAntiBotHoneyField(event.target.value)}
              />
            </div>

            <p className={`security-status ${isAntiBotReady ? 'ready' : 'pending'}`}>
              {antiBotStatusLabel}
            </p>
            {useTurnstile && (
              <div className="turnstile-block">
                <div id={TURNSTILE_CONTAINER_ID} className="turnstile-widget" />
                <p className="turnstile-hint">Valida Turnstile antes de iniciar la descarga.</p>
              </div>
            )}

            <div className="mode-switch" role="group" aria-label="Tipo de descarga">
              {(Object.keys(MODE_LABELS) as DownloadMode[]).map((item) => (
                <button
                  key={item}
                  type="button"
                  className={item === mode ? 'active' : ''}
                  onClick={() => setMode(item)}
                >
                  {MODE_LABELS[item]}
                </button>
              ))}
            </div>

            <label htmlFor="format-select">
              {mode === 'video' ? 'Resolucion (mejor a peor)' : 'Calidad de audio'}
            </label>
            <select
              id="format-select"
              value={selectedFormatId}
              onChange={handleFormatChange}
              disabled={options.length === 0}
            >
              {options.map((option: FormatOption) => (
                <option key={option.format_id} value={option.format_id}>
                  {option.label}
                </option>
              ))}
            </select>

            <button
              type="button"
              className="download-button"
              onClick={handleDownload}
              disabled={isDownloading || options.length === 0 || isLimitBlocked || !isAntiBotReady}
            >
              {isDownloading
                ? 'Descargando...'
                : isLimitBlocked
                  ? 'Limite diario alcanzado'
                  : isPreparingAntiBot || !isAntiBotReady
                    ? useTurnstile
                      ? 'Completa Turnstile'
                      : 'Verificando anti-bot...'
                    : 'Iniciar descarga'}
            </button>
          </form>

          {formats && (
            <article className="video-meta">
              {formats.thumbnail && (
                <img
                  className="thumbnail"
                  src={formats.thumbnail}
                  alt={`Miniatura de ${formats.title}`}
                  loading="lazy"
                />
              )}
              <div>
                <h2>{formats.title}</h2>
                <p>
                  {formats.video_options.length} opciones de video y {formats.audio_options.length}{' '}
                  opciones de audio detectadas.
                </p>
              </div>
            </article>
          )}

          {error && <p className="feedback error">{error}</p>}
          {notice && <p className="feedback ok">{notice}</p>}
        </section>

        <aside className="panel history-panel" id="historial">
          <div className="history-header">
            <div className="history-heading">
              <h2>Historial reciente</h2>
              <p>Ultimas 10 descargas</p>
            </div>
            <button
              type="button"
              className="history-clear-button"
              onClick={handleClearHistory}
              disabled={isClearingHistory || history.length === 0}
              aria-label="Borrar todo el historial"
              title="Borrar todo el historial"
            >
              <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                <path
                  d="M9 3h6l1 2h4v2H4V5h4l1-2Zm-2 6h2v9H7V9Zm4 0h2v9h-2V9Zm4 0h2v9h-2V9Z"
                  fill="currentColor"
                />
              </svg>
              <span>{isClearingHistory ? 'Borrando...' : 'Borrar'}</span>
            </button>
          </div>
          <ul>
            {history.length === 0 && <li className="empty">Todavia no hay descargas.</li>}
            {history.map((item) => (
              <li key={item.id} className="history-item">
                <div className="history-top">
                  <span className={`status ${item.status}`}>{item.status}</span>
                  <time dateTime={item.created_at}>{formatDate(item.created_at)}</time>
                </div>
                <div className="history-media">
                  {item.thumbnail ? (
                    <img
                      src={item.thumbnail}
                      alt={item.title ? `Miniatura de ${item.title}` : 'Miniatura del video'}
                      className="history-thumb"
                      loading="lazy"
                    />
                  ) : (
                    <div className="history-thumb history-thumb-placeholder" aria-hidden="true">
                      TD
                    </div>
                  )}
                  <p className="history-title">{item.title ?? 'Sin titulo detectado'}</p>
                </div>
                <p className="history-format">
                  {item.mode === 'video' ? 'Video' : 'Audio'} · {item.format}
                </p>
                <p className="history-url" title={item.url}>
                  {item.url}
                </p>
                {item.saved_path && <code>{item.saved_path}</code>}
                {item.error && <p className="history-error">{item.error}</p>}
              </li>
            ))}
          </ul>
        </aside>
      </main>

      <section className="legal-panels" aria-label="Secciones legales">
        <article className="panel legal-panel" id="privacidad">
          <h2>Privacidad</h2>
          <p>
            Total Downloader solo procesa la URL que envias para generar la descarga y mostrar el
            historial local reciente. No vendemos datos personales a terceros.
          </p>
        </article>

        <article className="panel legal-panel" id="terms">
          <h2>Terms</h2>
          <p>
            Al usar esta herramienta, aceptas cumplir los terminos de cada plataforma y descargar
            contenido unicamente cuando tengas permiso legal para hacerlo.
          </p>
        </article>
      </section>

      <footer className="site-footer" id="footer">
        <div className="footer-grid">
          <div className="footer-brand">
            <img src={logoSrc} alt="Total Downloader" className="footer-logo" />
            <p>
              Descargador profesional para video y audio. Rapido, ordenado y compatible con
              moviles, tablets y pantallas grandes.
            </p>
          </div>

          <div className="footer-column">
            <h3>Producto</h3>
            <ul>
              <li>
                <a href="#descargar">Descargar video</a>
              </li>
              <li>
                <a href="#descargar">Descargar audio</a>
              </li>
              <li>
                <a href="#historial">Historial</a>
              </li>
            </ul>
          </div>

          <div className="footer-column">
            <h3>Plataformas</h3>
            <ul>
              <li>YouTube</li>
              <li>TikTok</li>
              <li>Instagram</li>
              <li>Facebook / X</li>
            </ul>
          </div>

          <div className="footer-column">
            <h3>App</h3>
            <ul>
              <li>PWA instalable</li>
              <li>Modo responsive</li>
              <li>Fondo negro profesional</li>
            </ul>
            <button
              type="button"
              className="footer-install-button"
              onClick={handleInstallPwa}
              disabled={isPwaInstalled}
            >
              {isPwaInstalled ? 'App instalada' : 'Instalar PWA'}
            </button>
          </div>
        </div>

        <div className="footer-bottom">
          <p>© {currentYear} Total Downloader. Todos los derechos reservados.</p>
          <a href="#inicio">Volver arriba</a>
        </div>
      </footer>

      {limitRemainingSeconds !== null && (
        <div className="limit-modal-overlay" role="presentation">
          <div
            className="limit-modal"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="limit-title"
          >
            <h3 id="limit-title">Limite diario superado</h3>
            <p>
              Superaste el maximo de 10 descargas por IP en las ultimas 24 horas.
            </p>
            <p className="limit-countdown">{formatCountdown(limitRemainingSeconds)}</p>
            <p className="limit-hint">
              Podras volver a descargar cuando el contador llegue a <strong>00:00:00</strong>.
            </p>
            <button type="button" onClick={closeLimitModal}>
              Entendido
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

export default App
