import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'

// The launcher — the only local content in the app. Every instance is a
// remote webview added from Rust against its own origin (src-tauri/src/lib.rs).
// Port 5290 sits clear of the dev stack (5273/5274), MCP (5280), and the
// devbox app range (5301–5389); strictPort because tauri.conf.json's devUrl
// must not drift to whatever vite fell back to.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  // The launcher shares ui/'s dither engine instead of keeping a 649-line copy
  // in step by hand. The alias names the file (which imports nothing, so no
  // other alias is needed), and `fs.allow` lets vite serve it from outside this
  // package root.
  resolve: {
    alias: { '@dither': fileURLToPath(new URL('../ui/src/lib/dither-engine.ts', import.meta.url)) },
  },
  server: { port: 5290, strictPort: true, fs: { allow: ['..'] } },
  envPrefix: ['VITE_', 'TAURI_'],
})
