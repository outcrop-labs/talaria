<script lang="ts">
  import type { Snippet } from 'svelte'
  import { fade, pop, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import CloseButton from './CloseButton.svelte'

  // The one modal. Esc + backdrop-click close. Two shapes:
  //   • centered panel (default) — confirms, single-field creates, pickers.
  //     Sized by its content; if that outgrows the viewport the WHOLE dialog
  //     scrolls (see the scrollport comment below), so the header scrolls away
  //     with it. That is the trade the default shape makes.
  //   • `takeover` — fills the screen minus a padding gutter (the gutter is the
  //     "you're in a dialog" cue), constant height, content scrolls inside.
  //     Use for anything substantial: tabbed managers, libraries, composers.
  // Reach for `takeover` (or `height`) when the header and footer need to stay
  // pinned while the middle scrolls — that is the difference between them, not
  // merely how big the dialog looks.
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
    height = '',
    padded = true,
    takeover = false,
  }: {
    open: boolean
    onClose: () => void
    title?: string | Snippet
    children: Snippet
    footer?: Snippet
    width?: string
    /** Fixed-height dialogs (e.g. 'h-[85vh]' for the ticket detail): the frame
     *  becomes a column and the content region scrolls. */
    height?: string
    /** false: the content manages its own layout (multi-pane dialogs) — no p-7,
     *  no scroll wrapper. There is ONE modal primitive; big surfaces get these
     *  two knobs instead of hand-rolling their own shell. */
    padded?: boolean
    takeover?: boolean
  } = $props()
</script>

<svelte:document
  onkeydown={(e) => {
    if (open && e.key === 'Escape') onClose()
  }}
/>

{#snippet panel()}
  <div
    role="dialog"
    aria-modal="true"
    in:pop|global
    out:fade|global={QUICK}
    class={takeover
      ? 'relative z-10 flex h-full w-full flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]'
      : `relative z-10 w-full ${width} ${height} ${height ? 'flex flex-col overflow-hidden' : ''} rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]`}
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
    <div
      class={!padded
        ? 'min-h-0 flex-1 overflow-hidden'
        : takeover || height
          ? 'min-h-0 flex-1 overflow-y-auto p-7'
          : 'p-7'}
    >
      {@render children()}
    </div>
    {#if footer}
      <div class="shrink-0 border-t border-line px-7 py-4">{@render footer()}</div>
    {/if}
  </div>
{/snippet}

{#if open}
  <div use:portal class="fixed inset-0 z-50">
    <!-- |global on every leg: most call sites render {#if x}<SomeModal>, so
         this {#if open} block is created while an ANCESTOR mounts — local
         transitions (the default) are suppressed in exactly that case, which
         is how modals shipped two rounds of "completely unanimated". An
         overlay must animate no matter what mounted it. -->
    <div
      class="absolute inset-0 bg-black/50"
      in:fade|global={{ duration: 220 }}
      out:fade|global={QUICK}
      aria-hidden="true"
    ></div>
    {#if takeover}
      <!-- Constant height, content scrolls inside the panel. The gutter is both
           the "you're in a dialog" cue and the click-out target. -->
      <div role="presentation" class="absolute inset-0 p-6 sm:p-8" onclick={(e) => e.target === e.currentTarget && onClose()}>
        {@render panel()}
      </div>
    {:else}
      <!-- THE SCROLLPORT IS THIS OUTER LAYER, NOT THE PANEL.
           This was `grid place-items-center` on a `fixed inset-0` box with an
           unconstrained panel. A panel taller than the viewport then runs off
           the BOTTOM — measured: an 1811px dialog in a 513px viewport ended
           1312px past the fold — and because a `fixed` element contributes
           nothing to document scroll, nothing could bring it back. Every tall
           dialog lost its footer, which is where the submit button lives.
           Scrolling INSIDE the panel would fix the reach and break something
           else: EmojiPicker, DocLinkPopover and InfoTip are not portaled, so an
           overflow container here would clip them at the panel edge. Scrolling
           the whole dialog keeps them free.
           `min-h-full` + `items-center` is the pairing that matters: a short
           dialog centres, a tall one starts at its top and scrolls all the way
           down. Centring alone cannot do the second half. -->
      <div class="absolute inset-0 overflow-y-auto">
        <div
          role="presentation"
          class="flex min-h-full items-center justify-center p-4"
          onclick={(e) => e.target === e.currentTarget && onClose()}
        >
          {@render panel()}
        </div>
      </div>
    {/if}
  </div>
{/if}
