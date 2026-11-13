<script lang="ts" generics="T extends string">
  import type { Snippet } from 'svelte'
  import PageSurface from '@/components/app/PageSurface.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import { cn } from '@/lib/cn'
  import { staggerIn } from '@/lib/motion'
  import { setUiVoice } from '@/lib/ui-voice'
  import { Check } from '@lucide/svelte'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import { provideAutosave } from './autosave.svelte'

  // The settings area's page grammar: a title and one sentence, the tabs, then
  // the content. Every page in the area renders exactly one.
  let {
    title,
    blurb,
    tabs,
    tab,
    onTab,
    actions,
    autosave = false,
    saved = false,
    fill = false,
    staggerItems,
    children,
  }: {
    title: string
    /** One sentence under the title. */
    blurb?: string
    tabs?: ReadonlyArray<TabItem<T>>
    tab?: T
    onTab?: (id: T) => void
    /** Right of the title. At most one primary button. */
    actions?: Snippet
    /** The page's fields save on change, so the header says so. A field inside
     *  the page can ask for the same cue with `useAutosave()`. */
    autosave?: boolean
    /** Flash "Saved" in the cue, for a page that owns its own save. */
    saved?: boolean
    /** The content takes the window height left under the header, for a pane
     *  that owns its own scroll (a `LibraryPane`). */
    fill?: boolean
    /** A selector for the content's items, so `staggerIn` cascades them
     *  (its `data-stagger-items` contract). */
    staggerItems?: string
    children: Snippet
  } = $props()

  setUiVoice('sans')
  const cue = provideAutosave()
  // svelte-ignore state_referenced_locally -- reason: called during render, so it re-runs with fresh props on every render
  claimViewTitle(title)
</script>

<PageSurface innerClass={cn('max-w-[var(--settings-width)]', fill && 'h-full')}>
  <div use:staggerIn class={cn('font-sans', fill && 'flex h-full flex-col')}>
    <header class="flex shrink-0 flex-wrap items-start gap-x-6 gap-y-3">
      <div class="min-w-0 flex-1">
        <h1 class="font-sans text-[22px] font-semibold leading-7 tracking-[-0.01em] text-fg">{title}</h1>
        {#if blurb}<p class="mt-1 max-w-[72ch] font-sans text-sm leading-5 text-muted">{blurb}</p>{/if}
      </div>
      {#if autosave || cue.active || actions}
        <!-- Baselines, not centres: the cue sits on the line of a button's label, and with no button on the title's. -->
        <!-- On a phone the cue would squeeze the title to a sliver, so there the block takes its own line. -->
        <div
          class={cn(
            'flex shrink-0 flex-wrap items-baseline gap-x-4 gap-y-2',
            (autosave || cue.active) && 'max-sm:basis-full max-sm:justify-between',
          )}
        >
          {#if autosave || cue.active}
            <span role="status" class={cn('grid font-sans text-[13px] leading-5 text-muted', !actions && 'sm:mt-2')}>
              <!-- Holds the width of the longer line, so the flip to Saved moves nothing. -->
              <span aria-hidden="true" class="invisible col-start-1 row-start-1 whitespace-nowrap">Changes save automatically</span>
              <span class="col-start-1 row-start-1 flex items-center gap-1.5 whitespace-nowrap sm:justify-end">
                {#if saved || cue.saved}<Check size={14} aria-hidden="true" class="text-success" />Saved{:else}Changes save automatically{/if}
              </span>
            </span>
          {/if}
          {#if actions}<div class="flex flex-wrap items-center gap-2">{@render actions()}</div>{/if}
        </div>
      {/if}
    </header>
    {#if tabs && tab !== undefined && onTab}
      <Tabs items={tabs} value={tab} onChange={onTab} class="mt-5 shrink-0" />
    {/if}
    <div
      data-stagger-items={staggerItems}
      class={cn('space-y-8', tabs ? 'mt-6' : 'mt-8', fill && 'min-h-0 flex-1')}
    >
      {@render children()}
    </div>
  </div>
</PageSurface>
