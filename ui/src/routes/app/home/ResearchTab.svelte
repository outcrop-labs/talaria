<script lang="ts">
  import { navigate } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useResearchRuns } from '@/lib/research'
  import { RUN_DOT } from './home'

  // Research: runs in flight and fresh reports.
  const menu = useContextMenu()
  const list = listQuery(useResearchRuns(), { title: 'Could not load your research', variant: 'compact' })
</script>

<div class="space-y-6">
  <Panel>
    <SectionHeader title="Research" />
    {#if list.notice}
      <QueryError {...list.notice} />
    {/if}
    {#if list.pending}
      <SkeletonRows rows={5} />
    {:else if list.failed}
      <!-- the notice above already covers it -->
    {:else if list.rows.length === 0}
      <EmptyState variant="inline" title="Nothing researched yet." hint="Ask something on /research." />
    {:else}
      <ul class="divide-y divide-line">
        {#each list.rows.slice(0, 10) as r (r.id)}
          <li>
            <button
              type="button"
              onclick={() => void navigate('/research/:runId', { params: { runId: r.id } })}
              oncontextmenu={(e) =>
                menu.openMenu(e, [
                  { label: 'Open', onSelect: () => void navigate('/research/:runId', { params: { runId: r.id } }) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/research/${r.id}`) },
                ])}
              class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
            >
              <StatusDot status={RUN_DOT[r.status] ?? 'idle'} pulse={r.status === 'running'} />
              <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{r.question}</span>
              <Chip>{r.mode}</Chip>
              {#if r.status === 'error'}
                <span class="shrink-0 font-sans text-xs text-danger">failed: open to re-run →</span>
              {:else}
                <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{r.status}</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
  <ContextMenu {menu} />
</div>
