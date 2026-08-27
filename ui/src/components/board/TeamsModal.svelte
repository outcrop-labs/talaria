<script lang="ts">
  import { Pencil, Trash2 } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Modal from '@/components/ui/Modal.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirmDelete, prompt } from '@/components/ui/confirm.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { createTeam, deleteTeam, renameTeam, useTeams, type Team } from '@/lib/teams'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import TeamMembers from './TeamMembers.svelte'

  // Create teams and manage their members. Team members can access all of a team's
  // boards.
  let { open, onClose }: { open: boolean; onClose: () => void } = $props()

  const qc = useQueryClient()
  // `{ data: teams = [] }` turned a 500 on /api/teams into "Create a team to
  // get started" — an invitation to duplicate teams that already exist.
  const teamsList = listQuery(useTeams(), { title: 'Could not load your teams', variant: 'compact' })
  const teams = $derived(teamsList.rows)
  const teamsLoading = $derived(teamsList.pending)
  let selected = $state<string | null>(null)

  const team = $derived(teams.find((t) => t.id === selected) ?? teams[0] ?? null)
  const activeId = $derived(team?.id ?? null)

  const create = async (name: string) => {
    const { team: t } = await createTeam(name)
    await qc.invalidateQueries({ queryKey: ['teams'] })
    selected = t.id
  }

  const rename = async (t: Team) => {
    const name = await prompt({ title: 'Rename team', defaultValue: t.name, confirmLabel: 'Rename' })
    if (!name?.trim() || name === t.name) return
    try {
      await renameTeam(t.id, name.trim())
    } catch (e) {
      pushToast({ title: 'Rename failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    await qc.invalidateQueries({ queryKey: ['teams'] })
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
    if (selected === t.id) selected = null
    await qc.invalidateQueries({ queryKey: ['teams'] })
  }
</script>

<Modal {open} {onClose} title="Teams" width="max-w-2xl">
  <!-- The picker/detail shape is LibraryPane, shared with Templates and the
       agent role library. An inset WELL, not a panel: the Modal is already
       panel-filled, so a panel here would match its container exactly and read
       as a bare outline. -->
  <LibraryPane
    groups={[{ items: teams }]}
    idOf={(t: Team) => t.id}
    labelOf={(t: Team) => t.name}
    selectedId={activeId}
    onSelect={(t: Team) => (selected = t.id)}
    pending={teamsLoading}
    notice={teamsList.notice}
    listWidth="w-60"
    surface="well"
    class="min-h-[24rem]"
    onCreate={create}
    createLabel="New team"
  >
    {#snippet row(t: Team)}
      <span class="flex items-center justify-between gap-2">
        <span class="min-w-0 flex-1 truncate">{t.name}</span>
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{t.memberCount}</span>
      </span>
    {/snippet}

    {#snippet rowAction(t: Team)}
      <!-- Owner-only, like the server's gate: a member's row carries no verb
           the API would refuse. -->
      {#if t.role === 'owner'}
        <span class="flex gap-0.5">
          <IconButton size="sm" title="Rename {t.name}" onclick={() => void rename(t)}>
            <Pencil size={14} />
          </IconButton>
          <IconButton size="sm" danger title="Delete {t.name}" onclick={() => void remove(t)}>
            <Trash2 size={14} />
          </IconButton>
        </span>
      {/if}
    {/snippet}

    {#snippet empty()}
      {#if teamsLoading}
        <SkeletonRows rows={3} avatar class="w-full pt-1" />
      {:else if teamsList.failed}
        <!-- The team list is unknown, so "create one" is the wrong prompt —
             the left column already carries the error and its Retry. -->
        <p class="px-4 text-center font-sans text-sm text-muted">
          We couldn’t read your teams, so there’s nothing to show here yet.
        </p>
      {:else}
        <p class="font-sans text-sm text-muted">Create a team to get started.</p>
      {/if}
    {/snippet}

    {#snippet detail()}
      {#if team}
        <div class="min-h-0 flex-1 overflow-y-auto p-6">
          <TeamMembers teamId={activeId!} canManage={team.role === 'owner'} />
        </div>
      {/if}
    {/snippet}
  </LibraryPane>
</Modal>
