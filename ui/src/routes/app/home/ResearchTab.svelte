<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { researchRowMenu } from '@/components/record-menus'
  import { listQuery } from '@/components/ui/query-state'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { removeResearch, renameResearch, researchLabel, useResearchRuns } from '@/lib/research'
  import { useSession } from '@/lib/session'
  import { RUN_DOT } from './home'

  // Research: runs in flight and fresh reports. No archive — a run is
  // ephemeral, and delete (Remove) is already its stop.
  const menu = useContextMenu()
  const qc = useQueryClient()
  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const list = listQuery(useResearchRuns(), { title: 'Could not load your research', variant: 'compact' })
  const canManage = (r: { ownerUserId: string | null }) =>
    r.ownerUserId === session?.id || session?.role === 'admin'
  const refresh = () => void qc.invalidateQueries({ queryKey: ['research-runs'] })
</script>

<HomeListPanel
  title="Research"
  menu={menu}
  notice={list.notice}
  pending={list.pending}
  failed={list.failed}
  isEmpty={list.rows.length === 0}
  skeletonRows={5}
>
  {#snippet empty()}
    <EmptyState variant="inline" title="Nothing researched yet." hint="Ask something on /research." />
  {/snippet}
  {#each list.rows.slice(0, 10) as r (r.id)}
    <li>
      <button
        type="button"
        onclick={() => void navigate('/research/:runId', { params: { runId: r.id } })}
        oncontextmenu={(e) =>
          menu.openMenu(
            e,
            researchRowMenu(
              r.id,
              () => void navigate('/research/:runId', { params: { runId: r.id } }),
              canManage(r),
              {
                rename: () => {
                  void renameResearch(r.id, researchLabel(r)).then((ok) => {
                    if (ok) refresh()
                  })
                },
                remove: () => {
                  void removeResearch(r).then((ok) => {
                    if (ok) refresh()
                  })
                },
              },
            ),
          )}
        class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
      >
        <StatusDot status={RUN_DOT[r.status] ?? 'idle'} pulse={r.status === 'running'} />
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{researchLabel(r)}</span>
        <Chip>{r.mode}</Chip>
        {#if r.status === 'error'}
          <span class="shrink-0 font-sans text-xs text-danger">failed: open to re-run →</span>
        {:else}
          <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{r.status}</span>
        {/if}
      </button>
    </li>
  {/each}
</HomeListPanel>
