<script lang="ts">
  import type { Snippet } from 'svelte'
  import { X } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { closeTab, isClosable, selectTab, tabsFor, type WorkspaceTab } from '@/lib/workspace-tabs.svelte'

  // THE WORKSPACE PANE'S CHROME, and only that: the strip of tabs a session
  // has open, and the frame around whichever one is active. What a tab's body
  // renders — a brief, an artifact, a Google file, the run report, the drafts
  // waiting on a person — is the caller's business, handed in as a snippet
  // that receives the active tab. Keeping the bodies out of here is what lets
  // the strip stay one small file that every work surface can mount.
  let {
    sessionId,
    children,
  }: {
    sessionId: string | null
    /** Rendered with the active tab, or `null` when the session has none open
     *  yet — a session can exist before the agent has touched anything. */
    children: Snippet<[WorkspaceTab | null]>
  } = $props()

  const strip = $derived(tabsFor(sessionId))
</script>

{#if sessionId}
  <!-- `sessionId` is captured once here because the handlers below are
       closures: TypeScript stops narrowing a mutable prop inside a callback,
       and a const is also the honest thing to hand them. -->
  {@const id = sessionId}
  <div class="flex h-full min-h-0 flex-col">
    {#if strip.tabs.length > 0}
      <!-- Mono chrome voice, like every other tab strip in the app; the strip
           scrolls sideways rather than wrapping, so the body keeps its height
           no matter how many documents a long session touched. -->
      <div class="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-line px-2 py-1.5">
        {#each strip.tabs as tab (tab.id)}
          {@const active = strip.activeId === tab.id}
          <div class="flex shrink-0 items-center gap-0.5">
            <Chip
              title={tab.label}
              tone={active ? 'accent' : 'neutral'}
              onSelect={() => selectTab(id, tab.id)}
              class={active ? 'border-accent' : 'dither-fill'}
            >
              <!-- A document title is as long as somebody made it; the chip
                   truncates and the tooltip carries the whole thing. -->
              <span class="block max-w-[11rem] truncate">{tab.label}</span>
            </Chip>
            {#if isClosable(tab)}
              <IconButton
                title={`Close ${tab.label}`}
                size="sm"
                class={cn('h-5 w-5', active ? 'text-accent' : '')}
                onclick={() => closeTab(id, tab.id)}
              >
                <X size={11} strokeWidth={2} />
              </IconButton>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
    <!-- `min-h-0` keeps the height chain intact: without it a scrolling body
         pushes the strip off the top of the pane instead of scrolling. -->
    <div class="min-h-0 flex-1">
      {@render children(strip.active)}
    </div>
  </div>
{:else}
  <EmptyState
    icon="◫"
    title="No session open"
    hint="Pick a work session and its documents, artifacts and reports show up here."
  />
{/if}
