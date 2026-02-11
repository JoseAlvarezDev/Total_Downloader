

<h1 align="center">Total Downloader</h1>

<p align="center">
  Webapp moderna para descargar video o audio con <code>yt-dlp</code>, interfaz profesional y soporte PWA.
</p>

<p align="center">
  <a href="https://josealvarezdev.github.io/Total_Downloader/">
    <img src="https://img.shields.io/badge/Web-GitHub%20Pages-111827?style=for-the-badge&logo=githubpages&logoColor=white" alt="GitHub Pages" />
  </a>
  <a href="https://ko-fi.com/josealvarezdev">
    <img src="https://img.shields.io/badge/Apoya-Ko--fi-16a34a?style=for-the-badge&logo=kofi&logoColor=white" alt="Ko-fi" />
  </a>
</p>

<p align="center">
  <img src="./banner1280x640.png" alt="Total Downloader Banner" width="100%" />
</p>

## Plataformas soportadas
<p align="center">
  <img src="https://cdn.simpleicons.org/x/ffffff" alt="X" width="34" title="X" />
  &nbsp;&nbsp;
  <img src="https://cdn.simpleicons.org/facebook/1877F2" alt="Facebook" width="34" title="Facebook" />
  &nbsp;&nbsp;
  <img src="https://cdn.simpleicons.org/tiktok/ffffff" alt="TikTok" width="34" title="TikTok" />
  &nbsp;&nbsp;
  <img src="https://cdn.simpleicons.org/youtube/FF0000" alt="YouTube" width="34" title="YouTube" />
  &nbsp;&nbsp;
  <img src="https://cdn.simpleicons.org/instagram/E4405F" alt="Instagram" width="34" title="Instagram" />
  &nbsp;&nbsp;
  <img src="https://cdn.simpleicons.org/bluesky/0285FF" alt="Bluesky" width="34" title="Bluesky" />
</p>

## Caracteristicas
- Descarga en modo `Video` o `Audio`.
- Opciones de calidad/resolucion ordenadas de mejor a peor.
- Descarga directa al dispositivo desde el navegador.
- Historial reciente con miniatura y titulo (ultimas 10 descargas).
- Anti-bot: Turnstile o challenge PoW local de respaldo.
- Limite por IP: maximo 10 descargas por ventana de 24 horas.
- PWA instalable (desktop y movil).

## Stack
- Frontend: React + TypeScript + Vite
- Backend: Rust + Axum
- Motor de descarga: `yt-dlp` + `ffmpeg`
- Deploy frontend: GitHub Pages
- Deploy backend: Railway

## Demo
- URL publica: [https://josealvarezdev.github.io/Total_Downloader/](https://josealvarezdev.github.io/Total_Downloader/)

## Estructura del proyecto
- `frontend/` app React + Vite
- `backend/` API Rust
- `.github/workflows/deploy-pages.yml` deploy automatico de frontend
- `railway.json` configuracion de deploy backend en Railway

## Requisitos locales
- Node.js 20+
- Rust (stable)
- `yt-dlp`
- `ffmpeg`

macOS (Homebrew):

```bash
brew install yt-dlp ffmpeg
```

## Desarrollo local
### 1) Backend
```bash
cd backend
cargo run
```

Backend por defecto: `http://127.0.0.1:8787`

### 2) Frontend
```bash
cd frontend
npm install
npm run dev
```

Frontend por defecto: `http://127.0.0.1:5173`

## Variables de entorno
### Backend (produccion)
```bash
ALLOWED_ORIGINS=https://josealvarezdev.github.io
TRUST_PROXY_HEADERS=true
MAX_CONCURRENT_DOWNLOADS=3
TURNSTILE_SECRET_KEY=tu_secret_key_turnstile
```

- `ALLOWED_ORIGINS`: lista separada por comas de origenes permitidos para CORS.
- `TRUST_PROXY_HEADERS`: activar solo si hay proxy confiable delante.
- `MAX_CONCURRENT_DOWNLOADS`: descargas simultaneas maximas.
- `TURNSTILE_SECRET_KEY`: validacion anti-bot con Cloudflare Turnstile.

### Frontend (`frontend/.env`)
```bash
VITE_API_URL=https://totaldownloader-production.up.railway.app
VITE_TURNSTILE_SITE_KEY=tu_site_key_turnstile
```

## Deploy frontend (GitHub Pages)
El workflow publica `frontend/dist` en cada push a `main`.

Variables recomendadas en GitHub Actions (`Settings -> Secrets and variables -> Actions -> Variables`):
- `VITE_API_URL`
- `VITE_TURNSTILE_SITE_KEY`

## Deploy backend (Railway)
1. Crear proyecto desde el repo `JoseAlvarezDev/Total_Downloader`.
2. Usar builder `Dockerfile` con `Dockerfile` en raiz.
3. Configurar variables del backend.
4. Generar dominio publico y usarlo en `VITE_API_URL`.

Nota: el `Dockerfile` descarga `yt-dlp_linux` oficial para evitar problemas por versiones antiguas en paquetes del sistema.

## Persistencia local backend
- Historial: `backend/data/history.json`
- Limites por IP: `backend/data/rate_limits.json`
- Transferencias temporales: `backend/temp_downloads`

## API
- `GET /api/health`
- `GET /api/history`
- `DELETE /api/history`
- `GET /api/antibot/challenge`
- `POST /api/formats`
- `POST /api/download`

## SEO y archivos de descubrimiento
- `frontend/public/robots.txt`
- `frontend/public/sitemap.xml`
- `frontend/public/llm.txt`
- `frontend/public/llms.txt`

## Apoya el proyecto
- Ko-fi: [https://ko-fi.com/josealvarezdev](https://ko-fi.com/josealvarezdev)
