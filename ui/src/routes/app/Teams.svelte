<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate } from '@/router'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InlineCreate from '@/components/ui/InlineCreate.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirmDelete } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, openCopyItems } from '@/components/ui/context-menu.svelte'
  import { toastError } from '@/lib/toast.svelte'
  import { createTeam, deleteTeam, useTeamsAll, type Team } from '@/lib/teams'
  import TeamDetail, { teamCounts } from './TeamDetail.svelte'

  // View grant is the door — no admin-only gate here.

  const qc = useQueryClient()
  const read = listQuery(useTeamsAll(), { title: 'Could not load teams', variant: 'compact' })
  const teams = $derived(read.rows)
  const isLoading = $derived(read.pending)
  const failed = $derived(read.failed)
  const selectedId = $derived.by((): string | null => {
    const t = searchParams.get('t')
    return t == null || t === '' ? null : String(t)
  })
  const select = (id: string | null, replace = false) => {
    void navigate('/teams', { search: id ? { t: id } : {}, replace })
  }
  const selected = $derived(teams.find((t) => t.id === selectedId) ?? null)
  // The page never rests on an empty pane: with no valid selection, the first
  // team is opened. A REPLACE, so Back does not land on the unselected URL and
  // bounce forward again.
  $effect(() => {
    if (isLoading || failed || selected) return
    const first = teams[0]
    if (first) select(first.id, true)
  })
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
      toastError('Delete failed', e)
      return
    }
    // Refresh first: the selection effect then moves to the next team, and the
    // URL is only cleared when none is left.
    await refresh()
    if (selectedId === t.id) select(null)
  }
</script>

<SettingsPage title="Teams" blurb="Group people and agents so work and access follow the team." fill>
  {#snippet actions()}
    <InlineCreate label="New team" placeholder="Name of the team" onSubmit={(name) => void create(name)} />
  {/snippet}
  <div class="h-full min-h-[26rem]">
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
                ...openCopyItems(`/teams?t=${t.id}`, () => select(t.id)),
                'sep',
                { label: 'Delete', danger: true, onSelect: () => void remove(t) },
              ]
            : openCopyItems(`/teams?t=${t.id}`, () => select(t.id)),
        )}
      class="h-full"
    >
      {#snippet row(t: Team)}
        <span class="block truncate font-medium leading-5 text-fg">{t.name}</span>
        <span class="block truncate text-xs leading-4 text-muted">{teamCounts(t)}</span>
      {/snippet}

      {#snippet empty()}
        {#if isLoading}
          <div class="p-5" aria-hidden="true"><SkeletonRows rows={4} /></div>
        {:else}
          <EmptyState
            icon="▣"
            title={failed ? 'Teams could not be loaded' : 'No teams yet'}
            hint={failed ? 'Retry on the left.' : 'Create the first one with New team.'}
          />
        {/if}
      {/snippet}

      {#snippet detail()}
        {#if selected}
          {#key selected.id}
            <div class="min-h-0 flex-1 overflow-hidden">
              <TeamDetail team={selected} onChanged={refresh} onDelete={() => selected && void remove(selected)} />
            </div>
          {/key}
        {/if}
      {/snippet}
    </LibraryPane>
  </div>
  <ContextMenu {menu} />
</SettingsPage>
