<script lang="ts" generics="T extends string">
  import type { Snippet } from 'svelte'
  import PageSurface from '@/components/app/PageSurface.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import type { TabItem } from '@/components/ui/tabs'
  import { cn } from '@/lib/cn'
  import { staggerIn } from '@/lib/motion'
  import { setUiVoice } from '@/lib/ui-voice'
  import { claimViewTitle } from '@/lib/view-title.svelte'

  // The settings area's page grammar: a title and one sentence, the tabs, then
  // the content. Every page in the area renders exactly one.
  let {
    title,
    blurb,
    tabs,
    tab,
    onTab,
    actions,
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
    /** The content takes the window height left under the header, for a pane
     *  that owns its own scroll (a `LibraryPane`). */
    fill?: boolean
    /** A selector for the content's items, so `staggerIn` cascades them
     *  (its `data-stagger-items` contract). */
    staggerItems?: string
    children: Snippet
  } = $props()

  setUiVoice('sans')
  // svelte-ignore state_referenced_locally -- reason: called during render, so it re-runs with fresh props on every render
  claimViewTitle(title)
</script>

<PageSurface innerClass={fill ? 'h-full' : undefined}>
  <div use:staggerIn class={cn('font-sans', fill && 'flex h-full flex-col')}>
    <header class="flex shrink-0 flex-wrap items-start gap-x-6 gap-y-3">
      <div class="min-w-0 flex-1">
        <h1 class="font-sans text-[22px] font-semibold leading-7 tracking-[-0.01em] text-fg">{title}</h1>
        {#if blurb}<p class="mt-1 max-w-[72ch] font-sans text-sm leading-5 text-muted">{blurb}</p>{/if}
      </div>
      {#if actions}<div class="flex shrink-0 flex-wrap items-center gap-2">{@render actions()}</div>{/if}
    </header>
    {#if tabs && tab !== undefined && onTab}
      <Tabs items={tabs} value={tab} onChange={onTab} class="mt-5 shrink-0" />
    {/if}
    <div
      data-stagger-items={staggerItems}
      class={cn('space-y-10', tabs ? 'mt-6' : 'mt-8', fill && 'min-h-0 flex-1')}
    >
      {@render children()}
    </div>
  </div>
</PageSurface>
