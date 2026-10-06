<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import CountPill from '@/components/app/CountPill.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { conversationGlyph, conversationSelection, type SharedConversation } from '@/lib/comms-profile'
  import type { CommsSelection } from '@/lib/comms-selection'
  import type { QueryLike } from '@/components/ui/query-state'

  // The conversations shared with one person or agent, as rows: a glyph (`#`
  // channel, `⇄` relay) or the subject's avatar (their DM, an agent thread),
  // the name — bold while unread — the unread count and how long ago. The
  // profile drawer shows the first few (`dense`); /comms/with shows them all.
  let {
    query,
    limit,
    subjectName,
    subjectPicture,
    agentModel,
    onOpen,
    dense = false,
  }: {
    query: QueryLike<SharedConversation[]>
    limit?: number
    /** First name, for the empty line ("Nothing shared with Maya yet"). */
    subjectName: string
    /** The avatar a DM / agent-thread row wears: the person or agent's own. */
    subjectPicture?: string | null
    /** Set when the subject is an agent — its threads open under it. */
    agentModel: string | null
    onOpen: (sel: CommsSelection) => void
    dense?: boolean
  } = $props()
</script>

<QueryState {query} errorTitle="Could not load your conversations" errorVariant={dense ? 'compact' : 'full'}>
  {#snippet skeleton()}
    <div class={dense ? 'py-2' : 'mx-auto w-full max-w-[var(--converse-width)] p-5'}>
      <SkeletonRows rows={dense ? 3 : 5} avatar />
    </div>
  {/snippet}
  {#snippet empty()}
    <EmptyState
      variant={dense ? 'compact' : 'full'}
      icon="◈"
      title={`No conversations with ${subjectName} yet`}
      hint={agentModel
        ? 'Message them, or add them to a channel or relay you are in.'
        : 'Send a message, or add them to a channel or relay you are in.'}
    />
  {/snippet}
  {#snippet children(all)}
    {@const rows = limit ? all.slice(0, limit) : all}
    <ul class={cn(dense ? '-mx-2' : 'mx-auto w-full max-w-[var(--converse-width)] divide-y divide-line-subtle px-5 py-2')}>
      {#each rows as c (c.id)}
        {@const glyph = conversationGlyph(c.kind)}
        {@const target = conversationSelection(c, agentModel)}
        <li>
          <button
            type="button"
            disabled={!target}
            onclick={() => target && onOpen(target)}
            class={cn(
              'flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 text-left transition-colors hover:bg-card2',
              dense ? 'py-1.5' : 'py-3',
            )}
          >
            {#if glyph}
              <span class="grid h-5 w-5 shrink-0 place-items-center font-sans text-sm text-muted">{glyph}</span>
            {:else}
              <Avatar
                name={c.kind === 'dm' ? c.name : subjectName}
                src={subjectPicture}
                class="h-5 w-5 text-[9px]"
              />
            {/if}
            <span
              class={cn(
                'min-w-0 flex-1 truncate font-sans text-sm',
                c.unreadCount > 0 ? 'font-semibold text-fg' : 'text-fg',
              )}
            >
              {c.name}
            </span>
            <CountPill count={c.unreadCount} class="ml-0" />
            <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(c.lastAt)}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/snippet}
</QueryState>
