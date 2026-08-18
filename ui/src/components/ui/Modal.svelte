<script lang="ts">
  import type { Snippet } from 'svelte'
  import { fade, pop, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import CloseButton from './CloseButton.svelte'
  import FieldSurface from './FieldSurface.svelte'

  // The one modal. Esc + backdrop-click close. Two shapes:
  //   • centered panel (default) — confirms, single-field creates, pickers.
  //     Sized by its content, capped at the viewport.
  //   • `takeover` — fills the screen minus a padding gutter (the gutter is the
  //     "you're in a dialog" cue), constant height.
  //     Use for anything substantial: tabbed managers, libraries, composers.
  //
  // EVERY shape scrolls the same way: the panel is a flex column that cannot
  // exceed the viewport, the header and footer are pinned, and only the middle
  // scrolls. The shapes differ in how tall the panel is, never in whether the
  // chrome stays put — a dialog you cannot submit because its footer is off the
  // bottom of the screen is a broken dialog, at any size.
  //
  // If a default dialog routinely fills the viewport, that is a layout signal
  // rather than a scrolling one: group the related controls into tabs (see
  // BoardSettingsModal for the pattern) instead of letting one column grow.
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
      : `relative z-10 flex max-h-full w-full ${width} ${height} flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]`}
  >
  <!-- A MODAL OWNS ITS OWN SURFACE, for the same two reasons the rail does: it
       paints an opaque `bg-panel`, so a field drawn on the shell's canvas
       underneath would be hidden by it; and it sits above the shell in its own
       stacking context anyway. One canvas serves every control in the dialog. -->
  <FieldSurface class="flex min-h-0 flex-1 flex-col">
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
    <!-- The scrolling region, in EVERY shape. The panel is a flex column capped
         at the viewport (`max-h-full`, or an explicit `height`, or `h-full`
         under takeover), so this is the only part that can grow — the header
         and footer stay put and the submit button never leaves the screen.
         A dialog shorter than the window is unaffected: `max-h-full` sets a
         ceiling, not a height, so it still sizes to its content and centres. -->
    <div class={padded ? 'min-h-0 flex-1 overflow-y-auto p-7' : 'min-h-0 flex-1 overflow-hidden'}>
      {@render children()}
    </div>
    {#if footer}
      <div class="shrink-0 border-t border-line px-7 py-4">{@render footer()}</div>
    {/if}
  </FieldSurface>
  </div>
{/snippet}

{#if open}
  <div use:portal class={takeover ? 'fixed inset-0 z-50 p-6 sm:p-8' : 'fixed inset-0 z-50 grid place-items-center p-4'}>
    <!-- |global on every leg: most call sites render {#if x}<SomeModal>, so
         this {#if open} block is created while an ANCESTOR mounts — local
         transitions (the default) are suppressed in exactly that case, which
         is how modals shipped two rounds of "completely unanimated". An
         overlay must animate no matter what mounted it. -->
    <div
      class="absolute inset-0 bg-black/50"
      in:fade|global={{ duration: 220 }}
      out:fade|global={QUICK}
      onclick={onClose}
      aria-hidden="true"
    ></div>
    {@render panel()}
  </div>
{/if}
