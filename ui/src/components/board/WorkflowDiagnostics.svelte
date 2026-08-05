<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { getList } from '@/lib/fetch-json'
  import { cn } from '@/lib/cn'
  import { listStagger } from '@/lib/motion'

  /** A board-shape problem the server found, in its own words. Mirrors
   *  `StatusDiagnostic` in @/server/statuses — the sentences are composed THERE,
   *  from the same resolved fields the task engine refuses on, so this file never
   *  has to work out what makes a board broken. */
  interface StatusDiagnostic {
    level: 'error' | 'warning'
    text: string
  }

  /** The board's workflow, checked against what the engine can actually resolve.
   *
   *  WHY THIS EXISTS. Phase 0 made the resolvers refuse instead of guess: on a
   *  board whose only review column is also an agent-start queue, `reviewKey` is
   *  null and every agent hand-off throws a diagnostic naming the switch to flip.
   *  That diagnostic reaches the AGENT and the server log — not the board owner,
   *  who is the one person who can flip it, and who until now saw a perfectly
   *  tidy column list. `review + agentStart` rows predate the rule that refuses
   *  them and there is no migration, so a board can sit in that state for good.
   *  This panel is where those boards say so, to the person holding the checkbox. */
  let { boardId }: { boardId: string } = $props()

  const query = createQuery(() => ({
    queryKey: ['board-status-diagnostics', boardId],
    queryFn: () => getList<StatusDiagnostic>(`/api/boards/${boardId}/statuses`, 'diagnostics'),
  }))
  const list = listQuery(query, { title: 'Could not check this board’s workflow', variant: 'inline' })
</script>

{#if list.failed}
  {#if list.notice}<QueryError {...list.notice} />{/if}
{:else if !list.rows.length}
  {#if list.stale && list.notice}<QueryError {...list.notice} />{/if}
{:else}
  <div class="space-y-2" use:listStagger>
    {#if list.notice}<QueryError {...list.notice} />{/if}
    {#each list.rows as d, i (i)}
      <div
        class={cn(
          'rounded-lg border p-2.5 font-sans text-xs',
          d.level === 'error' ? 'border-danger/40 text-danger' : 'border-line text-muted',
        )}
      >
        <div class="mb-0.5 font-mono text-[10px] uppercase tracking-[0.08em]">
          {d.level === 'error' ? 'Agents are stuck on this board' : 'Probably not what you meant'}
        </div>
        <div class={d.level === 'error' ? 'text-fg' : undefined}>{d.text}</div>
      </div>
    {/each}
  </div>
{/if}
