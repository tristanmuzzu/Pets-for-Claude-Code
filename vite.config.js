import { defineConfig } from 'vite'
import { readFileSync } from 'node:fs'

const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))

// Tauri expects a fixed port and never wants Vite to wipe the terminal.
export default defineConfig({
  clearScreen: false,
  // Baked in rather than asked for at runtime: the update check needs to know
  // what it is comparing against before it has talked to anything.
  define: { __APP_VERSION__: JSON.stringify(version) },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] }
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
    emptyOutDir: true
  }
})
