<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronRight } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useUiVoice, type UiVoice } from '@/lib/ui-voice'

  // A clean collapsible section — collapsed by default. Reuse for thinking traces,
  // tool calls, and any other secondary detail that shouldn't clutter the thread.
  // Mercury (spec §10): compact mono trigger row on a raised hairline card.
  let {
    title,
    icon,
    defaultOpen = false,
    variant = 'compact',
    keepMounted = false,
    voice,
    class: className,
    footer,
    children,
  }: {
    title: string | Snippet
    icon?: Snippet
    defaultOpen?: boolean
    /** `card` is a card of a settings page whose trigger is a row of it. It
     *  applies in the sans voice only. */
    variant?: 'compact' | 'card'
    /** Keep the content in the document while closed, hidden, for content
     *  that must stay live: a status that keeps reading, a form mid-edit. */
    keepMounted?: boolean
    /** Wins over the voice the surrounding area set (`lib/ui-voice`). */
    voice?: UiVoice
    class?: string
    /** The foot of the open section, out to its edges under a hairline: a
     *  `SettingsSaveBar`, which brings its own padding. */
    footer?: Snippet
    children: Snippet
  } = $props()

  // svelte-ignore state_referenced_locally -- reason: initial open state per mount; callers wanting a different default remount the instance
  let open = $state(defaultOpen)

  const areaVoice = useUiVoice()
  const sans = $derived((voice ?? areaVoice) === 'sans')
  const card = $derived(sans && variant === 'card')
  const foot = $derived(card ? 'border-t border-line-subtle' : 'border-t border-line')
</script>

<div
  class={cn(
    card ? 'overflow-hidden rounded-lg border border-line bg-panel' : 'overflow-hidden rounded-md border border-line bg-card',
    className,
  )}
>
  <button
    type="button"
    onclick={() => (open = !open)}
    aria-expanded={sans ? open : undefined}
    class={card
      ? 'group/disc flex w-full cursor-pointer items-center gap-2.5 px-5 py-3.5 text-left font-sans text-sm font-medium leading-5 text-fg transition-colors hover:bg-hover'
      : sans
        ? 'group/disc flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left font-sans text-[13px] leading-5 text-muted transition-colors hover:bg-hover hover:text-fg'
        : 'group/disc flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left font-mono text-[11px] tracking-[0.05em] text-muted transition-colors dither-fill hover:text-fg'}
  >
    <!-- The chevron is the affordance, so it gets brighter on hover rather than
         staying the same dim glyph the row's own text is. A row that expands and
         a row that does nothing looked identical until the pointer was on it. -->
    {#if sans}
      <ChevronRight
        size={card ? 16 : 14}
        strokeWidth={1.5}
        class={cn('shrink-0 text-muted transition-transform duration-150 group-hover/disc:text-fg', open && 'rotate-90')}
      />
    {:else}
      <span class={cn('text-[10px] transition-all duration-150 group-hover/disc:text-accent', open && 'rotate-90')}>▶</span>
    {/if}
    {@render icon?.()}
    <span class="min-w-0 flex-1 truncate">
      {#if typeof title === 'string'}{title}{:else}{@render title()}{/if}
    </span>
  </button>
  {#if keepMounted}
    <div hidden={!open} class={card ? 'border-t border-line-subtle px-5 py-4' : 'border-t border-line px-3 py-2'}>
      {@render children()}
    </div>
    {#if footer}<div hidden={!open} class={foot}>{@render footer()}</div>{/if}
  {:else if open}
    <div transition:slide={{ duration: 150 }}>
      <div class={card ? 'border-t border-line-subtle px-5 py-4' : 'border-t border-line px-3 py-2'}>{@render children()}</div>
      {#if footer}<div class={foot}>{@render footer()}</div>{/if}
    </div>
  {/if}
</div>
