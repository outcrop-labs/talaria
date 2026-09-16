<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate } from '@/router'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirmDelete } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { staggerIn } from '@/lib/motion'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { createTeam, deleteTeam, useTeamsAll, type Team } from '@/lib/teams'
  import TeamDetail from './TeamDetail.svelte'

  // View grant is the door — no admin-only gate here.
  claimViewTitle('Teams', {
    info: 'People and agents as an organization principal. Membership expands at auth time onto shared work, views, and tools.',
  })

  const qc = useQueryClient()
  const read = listQuery(useTeamsAll(), { title: 'Could not load teams', variant: 'compact' })
  const teams = $derived(read.rows)
  const isLoading = $derived(read.pending)
  const failed = $derived(read.failed)
  const selectedId = $derived.by((): string | null => {
    const t = searchParams.get('t')
    return t == null || t === '' ? null : String(t)
  })
  const select = (id: string | null) => {
    void navigate('/teams', { search: id ? { t: id } : {} })
  }
  const selected = $derived(teams.find((t) => t.id === selectedId) ?? null)
  const menu = useContextMenu()

  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['teams'] }),
      qc.invalidateQueries({ queryKey: ['teams', 'all'] }),
    ])

  const create = async (name: string) => {
    const { team } = await createTeam(name)
    await refresh()
    if (team) select(team.id)
  }

  const remove = async (t: Team) => {
    if (
      !(await confirmDelete({
        what: 'team',
        name: t.name,
        detail: `Deleting “${t.name}” removes its member list. Its boards stay and become personal boards.`,
      }))
    )
      return
    try {
      await deleteTeam(t.id)
    } catch (e) {
      pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    if (selectedId === t.id) select(null)
    await refresh()
  }
</script>

<PageSurface>
  <div use:staggerIn>
    <div class="h-[calc(100vh-8rem)] min-h-[26rem]">
      <LibraryPane
        groups={[{ items: teams }]}
        idOf={(t: Team) => t.id}
        labelOf={(t: Team) => t.name}
        selectedId={selected?.id ?? null}
        onSelect={(t: Team) => select(t.id)}
        pending={isLoading}
        notice={read.notice}
        onRowMenu={(e: MouseEvent, t: Team) =>
          menu.openMenu(
            e,
            t.role === 'owner'
              ? [
                  { label: 'Open', onSelect: () => select(t.id) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/teams?t=${t.id}`) },
                  'sep',
                  { label: 'Delete', danger: true, onSelect: () => void remove(t) },
                ]
              : [
                  { label: 'Open', onSelect: () => select(t.id) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/teams?t=${t.id}`) },
                ],
          )}
        class="h-full"
        onCreate={create}
        createLabel="New team"
      >
        {#snippet row(t: Team)}
          <span class="flex items-center justify-between gap-2">
            <span class="min-w-0 flex-1 truncate">{t.name}</span>
            <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
              {t.memberCount} · {t.agentCount}
            </span>
          </span>
        {/snippet}

        {#snippet empty()}
          <EmptyState
            icon="▣"
            title="No team selected"
            hint={failed
              ? 'The team list could not be loaded. Retry on the left.'
              : teams.length
                ? 'Pick one on the left, or create a new one.'
                : 'Create the first one on the left.'}
          />
        {/snippet}

        {#snippet detail()}
          {#if selected}
            {#key selected.id}
              <div class="min-h-0 flex-1 overflow-hidden">
                <TeamDetail team={selected} onChanged={refresh} />
              </div>
            {/key}
          {/if}
        {/snippet}
      </LibraryPane>
    </div>
    <ContextMenu {menu} />
  </div>
</PageSurface>
