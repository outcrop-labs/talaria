<script lang="ts">
  import type { Snippet } from 'svelte'
  import { X } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { closeTab, isClosable, selectTab, tabsFor, type WorkspaceTab } from '@/lib/workspace-tabs.svelte'
  import FileGlyph from './FileGlyph.svelte'

  // THE WORKSPACE PANE'S CHROME, and only that: the strip of tabs a session
  // has open, and the frame around whichever one is active. What a tab's body
  // renders — a file, a plan's document, the run report, the drafts waiting on
  // a person — is the caller's business, handed in as a snippet that receives
  // the active tab. Keeping the bodies out of here is what lets the strip stay
  // one small file that every work surface can mount.
  let {
    sessionId,
    children,
    onClose,
    trailing,
  }: {
    sessionId: string | null
    /** Rendered with the active tab, or `null` when the session has none open
     *  yet — a session can exist before the agent has touched anything. */
    children: Snippet<[WorkspaceTab | null]>
    /** What closing a tab means, when it means more than leaving the strip (a
     *  session file is also HIDDEN for this viewer). Defaults to `closeTab`. */
    onClose?: (tab: WorkspaceTab) => void
    /** Quiet controls at the strip's end (the session's file list, the way to
     *  open another). Its presence also keeps the strip up when every tab is
     *  closed, so what was closed stays reachable. */
    trailing?: Snippet
  } = $props()

  const strip = $derived(tabsFor(sessionId))

  // The strip scrolls sideways, so the tab that takes the pane — the file the
  // agent just touched is usually the newest, at the far end — can sit past
  // the edge, active but out of sight. Whenever the active tab changes, slide
  // the strip just far enough to show it. Only the strip's own scrollLeft
  // moves: `scrollIntoView` would also scroll the page and the chat column.
  let scroller = $state<HTMLDivElement | null>(null)
  $effect(() => {
    const active = strip.activeId
    const box = scroller
    if (!active || !box) return
    const el = [...box.querySelectorAll<HTMLElement>('[data-tab-id]')].find((n) => n.dataset.tabId === active)
    if (!el) return
    const pad = 8
    const b = box.getBoundingClientRect()
    const t = el.getBoundingClientRect()
    if (t.left < b.left) box.scrollLeft -= b.left - t.left + pad
    else if (t.right > b.right) box.scrollLeft += t.right - b.right + pad
  })
</script>

{#if sessionId}
  <!-- `sessionId` is captured once here because the handlers below are
       closures: TypeScript stops narrowing a mutable prop inside a callback,
       and a const is also the honest thing to hand them. -->
  {@const id = sessionId}
  <div class="flex h-full min-h-0 flex-col">
    {#if strip.tabs.length > 0 || trailing}
      <div class="flex shrink-0 items-center border-b border-line">
        <!-- Mono chrome voice, like every other tab strip in the app; the
             strip scrolls sideways rather than wrapping, so the body keeps its
             height no matter how many files a long session touched. The
             trailing controls sit OUTSIDE the scroller so they never scroll
             away. -->
        <div bind:this={scroller} class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto px-2 py-1.5">
          {#each strip.tabs as tab (tab.id)}
            {@const active = strip.activeId === tab.id}
            <div data-tab-id={tab.id} class="flex shrink-0 items-center gap-0.5">
              <Chip
                title={tab.label}
                tone={active ? 'accent' : 'neutral'}
                onSelect={() => selectTab(id, tab.id)}
                class={cn('gap-1 py-0.5', active ? 'border-accent' : 'dither-fill')}
              >
                {#if tab.glyph}<FileGlyph glyph={tab.glyph} size={11} />{/if}
                <!-- A title is as long as somebody made it; the chip truncates
                     and the tooltip carries the whole thing. -->
                <span class="block max-w-[9rem] truncate">{tab.label}</span>
              </Chip>
              {#if isClosable(tab)}
                <IconButton
                  title={`Close ${tab.label}`}
                  size="sm"
                  class={cn('h-5 w-5', active ? 'text-accent' : '')}
                  onclick={() => (onClose ? onClose(tab) : closeTab(id, tab.id))}
                >
                  <X size={11} strokeWidth={2} />
                </IconButton>
              {/if}
            </div>
          {/each}
        </div>
        {#if trailing}
          <div class="flex shrink-0 items-center gap-1 pl-1 pr-2">{@render trailing()}</div>
        {/if}
      </div>
    {/if}
    <!-- `min-h-0` keeps the height chain intact: without it a scrolling body
         pushes the strip off the top of the pane instead of scrolling. -->
    <div class="min-h-0 flex-1">
      {@render children(strip.active)}
    </div>
  </div>
{:else}
  <EmptyState icon="◫" title="No session open" hint="Pick a session and its files show up here." />
{/if}
