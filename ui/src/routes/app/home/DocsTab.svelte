<script lang="ts">
  import { navigate } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useArtifacts } from '@/lib/artifacts'
  import { relativeTime } from '@/lib/fleet'

  // Docs: the latest durable output (artifacts incl. official KB mirrors).
  const menu = useContextMenu()
  const list = listQuery(useArtifacts(), { title: 'Could not load your documents', variant: 'compact' })
  const recent = $derived([...list.rows].sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1)).slice(0, 12))
</script>

<div class="space-y-6">
  <Panel>
    <SectionHeader title="Recently updated" />
    {#if list.notice}
      <QueryError {...list.notice} />
    {/if}
    {#if list.pending}
      <SkeletonRows rows={6} />
    {:else if list.failed}
      <!-- the notice above already covers it -->
    {:else if recent.length === 0}
      <EmptyState variant="inline" title="No documents yet." />
    {:else}
      <ul class="divide-y divide-line">
        {#each recent as a (a.id)}
          <li>
            <button
              type="button"
              onclick={() => void navigate('/artifacts', { search: { a: a.id } })}
              oncontextmenu={(e) =>
                menu.openMenu(e, [
                  { label: 'Open', onSelect: () => void navigate('/artifacts', { search: { a: a.id } }) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/artifacts?a=${a.id}`) },
                ])}
              class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
            >
              <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{a.title}</span>
              <Chip>{a.kind}</Chip>
              <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(a.updatedAt)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
  <ContextMenu {menu} />
</div>
