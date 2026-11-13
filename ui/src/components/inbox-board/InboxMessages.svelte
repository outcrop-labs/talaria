<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { InboxMessage } from '@/lib/inbox-board'
  import { fade, flip, LIST, QUICK, slide } from '@/lib/motion'
  import InboxMore from './InboxMore.svelte'
  import MessageRow from './MessageRow.svelte'
  import { ROW_GRID, SECTION, SECTION_COUNT, type TodayActions } from './inbox-board.svelte'

  let { messages, now, actions }: { messages: InboxMessage[]; now: number; actions: TodayActions } = $props()

  const SHOWN = 2

  let gone = $state<ReadonlySet<string>>(new Set())
  let showAll = $state(false)
  let replyingTo = $state<string | null>(null)

  const waiting = $derived(messages.filter((m) => !gone.has(m.line.current.id)))
  const folds = $derived(waiting.length > SHOWN + 1)
  const shown = $derived(showAll || !folds ? waiting : waiting.slice(0, SHOWN))
</script>

{#if waiting.length > 0}
  <section aria-labelledby="inbox-messages-title" class="@container overflow-hidden rounded-xl border border-line bg-panel">
    <header class="flex h-8 items-center gap-2 px-5">
      <h2 id="inbox-messages-title" class={cn(SECTION, 'mr-auto')}>
        Messages<span class={cn(SECTION_COUNT, 'ml-1.5')}>{waiting.length}</span>
      </h2>
      {#if folds}
        <InboxMore expanded={showAll} onclick={() => (showAll = !showAll)}>
          {showAll ? 'Show fewer' : `Show ${waiting.length - shown.length} more`}
        </InboxMore>
      {/if}
    </header>

    <ul class="@5xl:grid @5xl:grid-cols-[auto_fit-content(224px)_minmax(0,1fr)_3.5rem_auto] @5xl:gap-x-3">
      {#each shown as message (message.line.key)}
        {@const key = message.line.key}
        <li animate:flip={LIST} in:fade={QUICK} out:slide={{ duration: 160 }} class={cn('border-t border-line', ROW_GRID)}>
          <MessageRow
            {message}
            {now}
            {actions}
            replying={replyingTo === key}
            onReply={() => (replyingTo = replyingTo === key ? null : key)}
            onGone={() => (gone = new Set([...gone, message.line.current.id]))}
          />
        </li>
      {/each}
    </ul>
  </section>
{/if}
