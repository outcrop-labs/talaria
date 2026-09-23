<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { useContextMenu, openCopyItems } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useArtifacts } from '@/lib/artifacts'
  import { relativeTime } from '@/lib/fleet'

  // Docs: the latest durable output (artifacts incl. official KB mirrors).
  const menu = useContextMenu()
  const list = listQuery(useArtifacts(), { title: 'Could not load your documents', variant: 'compact' })
  const recent = $derived([...list.rows].sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1)).slice(0, 12))
</script>

<HomeListPanel
  title="Recently updated"
  menu={menu}
  notice={list.notice}
  pending={list.pending}
  failed={list.failed}
  isEmpty={recent.length === 0}
  skeletonRows={6}
>
  {#snippet empty()}
    <EmptyState variant="inline" title="No documents yet." />
  {/snippet}
  {#each recent as a (a.id)}
    <li>
      <button
        type="button"
        onclick={() => void navigate('/artifacts', { search: { a: a.id } })}
        oncontextmenu={(e) =>
          menu.openMenu(e, openCopyItems(`/artifacts?a=${a.id}`, () => void navigate('/artifacts', { search: { a: a.id } })))}
        class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
      >
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{a.title}</span>
        <Chip>{a.kind}</Chip>
        <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(a.updatedAt)}</span>
      </button>
    </li>
  {/each}
</HomeListPanel>