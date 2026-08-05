<script lang="ts">
  import type { Snippet } from 'svelte'
  import { fade, scale, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import CloseButton from './CloseButton.svelte'

  // The one modal. Esc + backdrop-click close. Two shapes:
  //   • centered panel (default) — confirms, single-field creates, pickers.
  //   • `takeover` — fills the screen minus a padding gutter (the gutter is the
  //     "you're in a dialog" cue), constant height, content scrolls inside.
  //     Use for anything substantial: tabbed managers, libraries, composers.
  //
  // Portaled to <body> via the portal action (see lib/portal.ts for why).
  //
  // Mercury (spec §8): panel surface + hairline + radius ~12 + matte shadow;
  // string titles render as mono uppercase labels (the panel-header voice).
  // Spec §9 entrance: ~160ms fade + small rise/scale, exit shorter; the
  // @/lib/motion wrappers drop the travel under reduced motion.
  let {
    open,
    onClose,
    title,
    children,
    footer,
    width = 'max-w-md',
    takeover = false,
  }: {
    open: boolean
    onClose: () => void
    title?: string | Snippet
    children: Snippet
    footer?: Snippet
    width?: string
    takeover?: boolean
  } = $props()
</script>

<svelte:document
  onkeydown={(e) => {
    if (open && e.key === 'Escape') onClose()
  }}
/>

{#if open}
  <div use:portal class={takeover ? 'fixed inset-0 z-50 p-6 sm:p-8' : 'fixed inset-0 z-50 grid place-items-center p-4'}>
    <div
      class="absolute inset-0 bg-black/50"
      transition:fade={QUICK}
      onclick={onClose}
      aria-hidden="true"
    ></div>
    <div
      role="dialog"
      aria-modal="true"
      in:scale={{ duration: 160, start: 0.98 }}
      out:fade={{ duration: 120 }}
      class={takeover
        ? 'relative z-10 flex h-full w-full flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]'
        : `relative z-10 w-full ${width} rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]`}
    >
      {#if title}
        <div class="flex shrink-0 items-center justify-between border-b border-line px-7 py-4">
          {#if typeof title === 'string'}
            <div class="min-w-0 truncate font-mono text-[11px] font-medium uppercase tracking-[0.08em] text-muted">{title}</div>
          {:else}
            <div class="min-w-0 text-sm font-semibold text-fg">{@render title()}</div>
          {/if}
          <CloseButton onClick={onClose} class="-mr-2" />
        </div>
      {/if}
      <div class={takeover ? 'min-h-0 flex-1 overflow-y-auto p-7' : 'p-7'}>{@render children()}</div>
      {#if footer}
        <div class="shrink-0 border-t border-line px-7 py-4">{@render footer()}</div>
      {/if}
    </div>
  </div>
{/if}
