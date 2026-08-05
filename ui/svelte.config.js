import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'

// Svelte 5 + Vite. vitePreprocess wires TypeScript (and PostCSS/Tailwind via
// Vite's pipeline) into <script lang="ts"> / <style> blocks.
export default {
  preprocess: vitePreprocess(),
}
