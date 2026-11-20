<script lang="ts">
  import { ExternalLink } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import type { SyncProblem } from '@/lib/board-sync'
  import { ticketSearch } from './board-settings'

  // The sync's CURRENT problems — what a person can act on now, not a log:
  // per ticket (an unmapped status, an assignee who is not on the board, a file
  // over the upload limit) and for the whole run (a refused credential, a list
  // that is gone, a held sweep). Each says what happened and the fix, and links
  // the ticket and the ClickUp task it is about. The projection carries the
  // first 50 and the total; this shows `shown` of them until asked for all.
  let {
    problems,
    total,
    boardId,
    shown = 5,
    onLeave,
  }: {
    problems: SyncProblem[]
    total: number
    boardId: string
    shown?: number
    /** Called before a ticket link navigates away (a popover closes itself). */
    onLeave?: () => void
  } = $props()

  let all = $state(false)
  const visible = $derived(all ? problems : problems.slice(0, shown))
  // The board's view and filters ride along (the ticket overlays the view it
  // was opened from); only the settings modal's own `?settings=` is dropped.
  const openTicket = (taskId: string) => {
    onLeave?.()
    void navigate('/boards/:boardId/:taskId', { params: { boardId, taskId }, search: ticketSearch(searchParams.toString()) })
  }
</script>

<ul class="space-y-1.5" data-sync-problems>
  {#each visible as p, i (`${p.kind}:${p.ticketId ?? ''}:${i}`)}
    <li class="rounded-md border border-line px-2.5 py-2" data-problem-kind={p.kind}>
      <div class="font-sans text-[13px] leading-snug text-fg">{p.detail}</div>
      <div class="mt-0.5 font-sans text-xs leading-snug text-muted">{p.fix}</div>
      {#if p.ticketId || p.clickupUrl}
        <div class="mt-1 flex flex-wrap items-center gap-3">
          {#if p.ticketId}
            <Button variant="link" class="text-xs" onclick={() => openTicket(p.ticketId!)}>
              {p.ticketRef ?? 'Open the ticket'}
            </Button>
          {/if}
          {#if p.clickupUrl}
            <a
              href={p.clickupUrl}
              target="_blank"
              rel="noopener noreferrer"
              class="inline-flex items-center gap-1 font-sans text-xs text-accent underline-offset-2 hover:underline"
            >
              Open in ClickUp <ExternalLink size={11} />
            </a>
          {/if}
        </div>
      {/if}
    </li>
  {/each}
</ul>
{#if !all && problems.length > visible.length}
  <Button variant="link" class="mt-1.5 text-xs" onclick={() => (all = true)}>Show all {total}</Button>
{:else if total > problems.length && (all || problems.length <= shown)}
  <div class="mt-1.5 font-sans text-xs text-muted">
    The first {problems.length} of {total} are shown; each clears on the next sync that resolves it.
  </div>
{/if}
