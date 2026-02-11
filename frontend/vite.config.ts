import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { VitePWA } from 'vite-plugin-pwa'

function normalizeBasePath(value: string): string {
  const prefixed = value.startsWith('/') ? value : `/${value}`
  return prefixed.endsWith('/') ? prefixed : `${prefixed}/`
}

const repositoryName = process.env.GITHUB_REPOSITORY?.split('/')[1] ?? 'Total_Downloader'
const defaultBasePath = process.env.GITHUB_ACTIONS === 'true' ? `/${repositoryName}/` : '/'
const basePath = normalizeBasePath(process.env.VITE_BASE_PATH ?? defaultBasePath)

// https://vite.dev/config/
export default defineConfig({
  base: basePath,
  plugins: [
    react(),
    VitePWA({
      registerType: 'autoUpdate',
      injectRegister: 'auto',
      includeAssets: ['image.png', 'apple-touch-icon.png'],
      manifest: {
        name: 'Total Downloader',
        short_name: 'TotalDL',
        description:
          'Descargador de videos y audio para X, Facebook, TikTok, YouTube, Instagram y Bluesky.',
        theme_color: '#060606',
        background_color: '#060606',
        display: 'standalone',
        orientation: 'portrait',
        scope: basePath,
        start_url: basePath,
        lang: 'es-ES',
        categories: ['utilities', 'productivity', 'multimedia'],
        icons: [
          {
            src: 'pwa-192x192.png',
            sizes: '192x192',
            type: 'image/png',
            purpose: 'any',
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'any',
          },
        ],
        screenshots: [
          {
            src: 'banner1280x640.png',
            sizes: '1280x640',
            type: 'image/png',
            form_factor: 'wide',
          },
        ],
      },
      workbox: {
        globPatterns: ['**/*.{js,css,html,png,svg,ico,webmanifest}'],
        cleanupOutdatedCaches: true,
      },
    }),
  ],
})
