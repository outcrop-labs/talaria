<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useSavedCue } from '@/components/admin/saved-cue'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { firstNotice, listQuery } from '@/components/ui/query-state'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { slide } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import { useUsers } from '@/lib/users'
  import { useTeams } from '@/lib/teams'
  import { delJsonGone, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import RowStatus from './RowStatus.svelte'
  import ToggleField from './ToggleField.svelte'
  import { indexState, type Binding, type IndexedCollection, type RagCollection } from './retrieval'

  let {
    col,
    spaces,
    index,
  }: {
    col: RagCollection
    spaces: Array<{ id: string; name: string; collectionId: string | null }>
    /** The collection as the vector store holds it. Absent while that read is
     *  out or failed, and then the row claims nothing about its index. */
    index?: IndexedCollection
  } = $props()

  const qc = useQueryClient()
  const autosave = useSavedCue()
  const agentsQuery = useAgents()
  const fleet = $derived(agentsQuery.data)
  // These two feed the pickers that decide WHO CAN RETRIEVE this collection.
  // Defaulted to `[]` they turned a failed directory read into a picker with no
  // options and existing grants shown as unresolved ids — an access-control
  // surface quietly reporting that the people it grants to do not exist.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const teamsList = listQuery(useTeams(), { title: 'Could not load teams', variant: 'inline' })
  const users = $derived(usersList.rows)
  const teams = $derived(teamsList.rows)
  // Until the directories land the comboboxes would misrepresent the bindings
  // (zero options, nothing selected) — hold their slots with bars instead.
  // Every read's flag is read before they are combined (query-state.ts,
  // anyLoading): `agentsQuery.isPending || usersList.pending || …` left the
  // directories' flags unread while the agents loaded, and the notices below
  // read only their `isError`, so directories that answered first told this
  // row nothing and the pickers stayed on their bars for good.
  const pickersPending = $derived([agentsQuery.isPending, usersList.pending, teamsList.pending].includes(true))
  const bindingsAll = $derived(col.bindings.some((b) => b.principalType === 'all'))
  const boundUsers = $derived(col.bindings.filter((b) => b.principalType === 'user').map((b) => b.principalId!).filter(Boolean))
  const boundAgents = $derived(col.bindings.filter((b) => b.principalType === 'agent').map((b) => b.principalId!).filter(Boolean))
  const boundTeams = $derived(col.bindings.filter((b) => b.principalType === 'team').map((b) => b.principalId!).filter(Boolean))
  const feedingSpaces = $derived(spaces.filter((s) => s.collectionId === col.id).map((s) => s.id))

  const setBindings = async (bindings: Binding[]) => {
    try {
      await putJson(`/api/rag/collections/${col.id}`, { bindings })
    } catch (e) {
      toastError('Save bindings failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
    autosave.flash()
  }
  const del = async () => {
    if (!(await confirm({ title: 'Delete collection', message: `Delete the "${col.name}" collection and its index?`, confirmLabel: 'Delete', danger: true }))) return
    try {
      // A collection another admin already deleted is a 404: gone, as asked.
      await delJsonGone(`/api/rag/collections/${col.id}`)
    } catch (e) {
      toastError('Delete collection failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const setSpaces = async (ids: string[]) => {
    // Diff: newly picked spaces bind to this collection; removed ones unbind.
    const added = ids.filter((id) => !feedingSpaces.includes(id))
    const removed = feedingSpaces.filter((id) => !ids.includes(id))
    try {
      for (const spaceId of added) {
        await putJson('/api/admin/rag', { spaceBrain: { spaceId, collectionId: col.id } })
      }
      for (const spaceId of removed) {
        await putJson('/api/admin/rag', { spaceBrain: { spaceId, collectionId: null } })
      }
    } catch (e) {
      toastError('Save spaces failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
    autosave.flash()
  }

  const applyBindings = (opts: { all?: boolean; users?: string[]; agents?: string[]; teams?: string[] }) => {
    const all = opts.all ?? bindingsAll
    const next: Binding[] = []
    if (all) next.push({ principalType: 'all', principalId: null })
    for (const id of opts.users ?? boundUsers) next.push({ principalType: 'user', principalId: id })
    for (const id of opts.agents ?? boundAgents) next.push({ principalType: 'agent', principalId: id })
    for (const id of opts.teams ?? boundTeams) next.push({ principalType: 'team', principalId: id })
    void setBindings(next)
  }

  let managing = $state(false)
  const idx = $derived(index ? indexState(index) : null)
  const entries = $derived(
    index && !index.missing ? `${index.pointsCount.toLocaleString()} ${index.pointsCount === 1 ? 'entry' : 'entries'}` : null,
  )
  // Auto collections have fixed access (activity = your own scope; org-kb =
  // everyone). Custom collections are bound and curated in the dialog.
  const menu: ContextMenuEntry[] = [
    { label: 'Manage', onSelect: () => (managing = true) },
    'sep',
    { label: 'Delete collection', danger: true, onSelect: () => void del() },
  ]
</script>

<div>
  <SettingsRow label={col.name} onOpen={col.auto ? undefined : () => (managing = true)} openLabel={`Manage ${col.name}`}>
    {#snippet meta()}{#if col.auto}<Chip>Built-in</Chip>{/if}{/snippet}
    {#snippet hint()}
      {#if entries}<span class="tabular-nums">{entries}</span>{#if col.description}{' · '}{/if}{/if}{col.description ?? ''}
    {/snippet}
    {#snippet status()}{#if idx}<RowStatus status={idx.status}>{idx.word}</RowStatus>{/if}{/snippet}
    {#if !col.auto}<SettingsRowMenu label={`Actions for ${col.name}`} items={menu} />{/if}
  </SettingsRow>
{#if !col.auto}
  <Modal open={managing} onClose={() => (managing = false)} title={col.name} width="max-w-lg">
    <div class="space-y-5">
      <ToggleField
        label="Everyone can search"
        hint="Turn off to choose the teams, people and agents who can."
        checked={bindingsAll}
        onChange={(checked) => applyBindings({ all: checked })}
      />
      {#if !bindingsAll && pickersPending}
        <div class="space-y-5">
          {#each [0, 1, 2] as i (i)}<Skeleton class="h-9 w-full" />{/each}
        </div>
      {/if}
      {#if !bindingsAll && !pickersPending}
        <div transition:slide={{ duration: 150 }} class="space-y-5">
          <SettingsField label="Teams">
            <Combobox
              options={teams.map((t) => ({ value: t.id, label: t.name }))}
              selected={boundTeams}
              onChange={(ids) => applyBindings({ teams: ids })}
              multiple
              size="sm"
              placeholder="No teams"
            />
          </SettingsField>
          <SettingsField label="People">
            <Combobox
              options={users.map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id }))}
              selected={boundUsers}
              onChange={(ids) => applyBindings({ users: ids })}
              multiple
              size="sm"
              placeholder="No people"
            />
          </SettingsField>
          <SettingsField label="Agents">
            <Combobox
              options={(fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label }))}
              selected={boundAgents}
              onChange={(ids) => applyBindings({ agents: ids })}
              multiple
              size="sm"
              placeholder="No agents"
            />
          </SettingsField>
        </div>
      {/if}
      {#if !bindingsAll && firstNotice(teamsList.notice, usersList.notice)}
        <div transition:slide={{ duration: 150 }} class="space-y-1">
          {#if teamsList.notice}<QueryError {...teamsList.notice} />{/if}
          {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
        </div>
      {/if}
      <SettingsField label="Knowledge spaces" hint="Documents in these spaces are indexed into this collection.">
        <Combobox
          options={spaces.map((s) => ({ value: s.id, label: s.name }))}
          selected={feedingSpaces}
          onChange={(ids) => void setSpaces(ids)}
          multiple
          size="sm"
          placeholder="No spaces"
        />
      </SettingsField>
    </div>
    {#snippet footer()}
      <div class="flex items-center gap-2">
        <DangerLink
          onClick={() => {
            managing = false
            void del()
          }}
        >
          Delete collection
        </DangerLink>
        <span class="flex-1"></span>
        <Button size="sm" variant="outline" onclick={() => (managing = false)}>Done</Button>
      </div>
    {/snippet}
  </Modal>
{/if}
</div>
