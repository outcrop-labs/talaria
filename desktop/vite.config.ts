import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// The launcher — the only local content in the app. Every instance is a
// remote webview added from Rust against its own origin (src-tauri/src/lib.rs).
// Port 5290 sits clear of the dev stack (5273/5274), MCP (5280), and the
// devbox app range (5301–5389); strictPort because tauri.conf.json's devUrl
// must not drift to whatever vite fell back to.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  server: { port: 5290, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_'],
})
