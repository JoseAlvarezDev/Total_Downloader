# Total Downloader

Webapp moderna para descargar videos o audio desde X, Facebook, TikTok, YouTube, Instagram y otras plataformas soportadas por `yt-dlp`.

## Apoya el proyecto

Si te gusta Total Downloader y quieres apoyar su desarrollo:

- Ko-fi: [https://ko-fi.com/josealvarezdev](https://ko-fi.com/josealvarezdev)

## Stack

- Frontend: React + TypeScript + Vite
- Backend: Rust + Axum
- Motor de descarga: `yt-dlp` (y `ffmpeg` para extracción de audio)

## Funcionalidades

- Input de URL para cargar formatos disponibles.
- Descarga en modo `Video` o `Audio`.
- Opciones de calidad/resolución ordenadas de mejor a peor.
- Descarga directa al dispositivo del usuario (navegador).
- Filtro anti-bot (challenge PoW + honeypot) antes de cada descarga.
- Límite anti-bot: máximo 10 descargas por IP en una ventana de 24 horas.
- Historial persistente de las últimas 10 descargas por IP.
- UI profesional con fondo negro y diseño responsive.
- PWA instalable (manifest + service worker) con botón `Descargar app (PWA)`.

## Estructura

- `/Users/josealvarez/Desktop/Total_Downloader/frontend` app React.
- `/Users/josealvarez/Desktop/Total_Downloader/backend` API Rust.

## Requisitos

- Node.js 20+
- Rust (stable)
- `yt-dlp`
- `ffmpeg`

### macOS (Homebrew)

```bash
brew install yt-dlp ffmpeg
```

## Ejecutar en desarrollo

### 1) Backend (Rust)

```bash
cd /Users/josealvarez/Desktop/Total_Downloader/backend
cargo run
```

Servidor por defecto: `http://127.0.0.1:8787`

Variables recomendadas para producción:

```bash
ALLOWED_ORIGINS=https://tu-frontend.com
TRUST_PROXY_HEADERS=false
MAX_CONCURRENT_DOWNLOADS=3
TURNSTILE_SECRET_KEY=tu_secret_key_turnstile
```

- `ALLOWED_ORIGINS`: lista separada por comas de orígenes permitidos para CORS.
- `TRUST_PROXY_HEADERS`: solo usa `true` si tienes un proxy confiable delante (Cloudflare/Nginx bien configurado).
- `MAX_CONCURRENT_DOWNLOADS`: límite de descargas simultáneas en el backend.
- `TURNSTILE_SECRET_KEY`: activa validación anti-bot con Cloudflare Turnstile. Si no se define, el backend usa PoW local como fallback.

### 2) Frontend (Vite)

```bash
cd /Users/josealvarez/Desktop/Total_Downloader/frontend
npm install
npm run dev
```

Frontend por defecto: `http://127.0.0.1:5173`

## Deploy automático a GitHub Pages

Este repo incluye el workflow:

- `/Users/josealvarez/Desktop/Total_Downloader/.github/workflows/deploy-pages.yml`

Se ejecuta en cada push a `main` y publica `frontend/dist` en GitHub Pages.

### Variables recomendadas en GitHub (Settings -> Secrets and variables -> Actions -> Variables)

- `VITE_API_URL`: URL pública de tu backend (ejemplo: `https://api.tudominio.com`)
- `VITE_TURNSTILE_SITE_KEY`: Site key pública de Cloudflare Turnstile

Si `VITE_API_URL` no está configurada, el frontend intentará `http://127.0.0.1:8787`, que no funcionará en producción.

### CORS para Pages

En backend, `ALLOWED_ORIGINS` debe incluir el dominio de Pages:

```bash
ALLOWED_ORIGINS=https://josealvarez.github.io,http://127.0.0.1:5173
```

## Backend en Railway

Pasos para desplegar la API Rust en Railway:

1. En Railway, crea `New Project` -> `Deploy from GitHub repo`.
2. Selecciona el repo `JoseAlvarezDev/Total_Downloader`.
3. En el servicio creado, configura:
   - `Root Directory`: `backend`
   - (si te lo pide) usa `Dockerfile` como método de build
4. Activa un dominio público (`Networking` -> `Generate Domain`).
5. En `Variables` del servicio Railway, define:
   - `TRUST_PROXY_HEADERS=true`
   - `MAX_CONCURRENT_DOWNLOADS=3`
   - `ALLOWED_ORIGINS=https://josealvarez.github.io`
   - `TURNSTILE_SECRET_KEY=<tu_secret_key_de_cloudflare>`
6. Espera a que el deploy termine y copia la URL pública (ejemplo: `https://total-downloader-api.up.railway.app`).
7. En GitHub -> `Settings` -> `Secrets and variables` -> `Actions` -> `Variables`, actualiza:
   - `VITE_API_URL=https://tu-url-publica-de-railway`
8. Haz un push a `main` o relanza el workflow de Pages para que el frontend use la nueva API.

## Instalar como PWA

- En Chrome/Edge: abre la web y usa el botón `Instalar app` de la barra de direcciones.
- En Safari iOS: `Compartir` -> `Agregar a pantalla de inicio`.

## Configuración del frontend

Si necesitas cambiar la URL del backend, crea `frontend/.env`:

```bash
VITE_API_URL=http://127.0.0.1:8787
VITE_TURNSTILE_SITE_KEY=tu_site_key_turnstile
```

- `VITE_TURNSTILE_SITE_KEY`: habilita el widget Turnstile en frontend. Si no se define, el frontend usa PoW local.

## Persistencia local

El backend guarda:

- Historial: `/Users/josealvarez/Desktop/Total_Downloader/backend/data/history.json`
- Límites por IP: `/Users/josealvarez/Desktop/Total_Downloader/backend/data/rate_limits.json`
- Archivos temporales de transferencia: `/Users/josealvarez/Desktop/Total_Downloader/backend/temp_downloads`

## Endpoints API

- `GET /api/health`
- `GET /api/history`
- `DELETE /api/history`
- `GET /api/antibot/challenge`
- `POST /api/formats`
- `POST /api/download`
