<script lang="ts">
  import type { Snippet } from 'svelte'
  import { X } from '@lucide/svelte'
  import { portal } from '@/lib/portal'

  let { title, onClose, children }: { title: string; onClose: () => void; children: Snippet } = $props()
</script>

<svelte:document
  onkeydown={(event) => {
    if (event.key === 'Escape') onClose()
  }}
/>

<div use:portal class="fixed inset-0 z-50" role="dialog" aria-modal="true" aria-label="{title} drawer">
  <button type="button" aria-label="Close drawer" onclick={onClose} class="absolute inset-0 bg-black/50"></button>
  <aside class="gd-enter absolute inset-y-0 right-0 flex w-full max-w-[560px] flex-col border-l border-line bg-surface shadow-[var(--theme-shadow-3)]">
    <header class="flex h-14 shrink-0 items-center border-b border-line px-5">
      <h2 class="font-mono text-[11px] uppercase tracking-[0.08em] text-fg">{title}</h2>
      <button type="button" onclick={onClose} class="ml-auto grid h-8 w-8 place-items-center rounded-md text-muted hover:bg-hover hover:text-fg" aria-label="Close {title}"><X size={15} /></button>
    </header>
    <div class="min-h-0 flex-1 overflow-y-auto p-5">{@render children()}</div>
  </aside>
</div>
