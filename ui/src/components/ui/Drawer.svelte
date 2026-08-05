<script lang="ts">
  import type { Snippet } from 'svelte'
  import { fade, fly, PANEL_X, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import CloseButton from './CloseButton.svelte'

  // The one drawer: a right-edge sheet, Modal's sibling. Esc + backdrop-click
  // close, portaled to <body>, entrance/exit per the drawer grammar
  // (ANIMATIONS.md: in:fly={PANEL_X} toward resting place, out:fade={QUICK}).
  // No component hand-rolls its own fixed-inset shell — content goes in here.
  //
  // Mercury: surface + hairline left border + matte shadow; string titles
  // render as mono uppercase labels (the panel-header voice), same as Modal.
  let {
    open,
    onClose,
    title,
    children,
    width = 'max-w-[560px]',
  }: {
    open: boolean
    onClose: () => void
    title?: string | Snippet
    children: Snippet
    width?: string
  } = $props()
</script>

<svelte:document
  onkeydown={(e) => {
    if (open && e.key === 'Escape') onClose()
  }}
/>

{#if open}
  <div use:portal class="fixed inset-0 z-50">
    <!-- |global on every leg, same reasoning as Modal.svelte: call sites render
         {#if x}<SomeDrawer>, so this block is created while an ANCESTOR mounts
         and local transitions are suppressed. An overlay must animate no
         matter what mounted it. -->
    <div
      class="absolute inset-0 bg-black/50"
      in:fade|global={{ duration: 220 }}
      out:fade|global={QUICK}
      onclick={onClose}
      aria-hidden="true"
    ></div>
    <div
      role="dialog"
      aria-modal="true"
      aria-label={typeof title === 'string' ? title : undefined}
      in:fly|global={PANEL_X}
      out:fade|global={QUICK}
      class="absolute inset-y-0 right-0 flex w-full {width} flex-col border-l border-line bg-surface shadow-[var(--theme-shadow-3)]"
    >
      {#if title}
        <header class="flex h-14 shrink-0 items-center border-b border-line px-5">
          {#if typeof title === 'string'}
            <h2 class="min-w-0 truncate font-mono text-[11px] uppercase tracking-[0.08em] text-fg">{title}</h2>
          {:else}
            <div class="min-w-0 text-sm font-semibold text-fg">{@render title()}</div>
          {/if}
          <CloseButton onClick={onClose} class="ml-auto -mr-2" size={15} label="Close {typeof title === 'string' ? title : 'drawer'}" />
        </header>
      {/if}
      <div class="min-h-0 flex-1 overflow-y-auto p-5">{@render children()}</div>
    </div>
  </div>
{/if}
