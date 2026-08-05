<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { confirm } from '@/components/ui/confirm.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { useAgents } from '@/lib/agents'
  import { useUsers } from '@/lib/users'
  import { useTeams } from '@/lib/teams'
  import type { Binding, RagCollection } from './retrieval'

  let {
    col,
    spaces,
  }: { col: RagCollection; spaces: Array<{ id: string; name: string; collectionId: string | null }> } = $props()

  const qc = useQueryClient()
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
  const pickersPending = $derived(agentsQuery.isPending || usersList.pending || teamsList.pending)
  const bindingsAll = $derived(col.bindings.some((b) => b.principalType === 'all'))
  const boundUsers = $derived(col.bindings.filter((b) => b.principalType === 'user').map((b) => b.principalId!).filter(Boolean))
  const boundAgents = $derived(col.bindings.filter((b) => b.principalType === 'agent').map((b) => b.principalId!).filter(Boolean))
  const boundTeams = $derived(col.bindings.filter((b) => b.principalType === 'team').map((b) => b.principalId!).filter(Boolean))
  const feedingSpaces = $derived(spaces.filter((s) => s.collectionId === col.id).map((s) => s.id))

  const setBindings = async (bindings: Binding[]) => {
    await fetch(`/api/rag/collections/${col.id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ bindings }) })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const del = async () => {
    if (!(await confirm({ title: 'Delete collection', message: `Delete the "${col.name}" collection and its index?`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/rag/collections/${col.id}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const setSpaces = async (ids: string[]) => {
    // Diff: newly picked spaces bind to this brain; removed ones unbind.
    const added = ids.filter((id) => !feedingSpaces.includes(id))
    const removed = feedingSpaces.filter((id) => !ids.includes(id))
    for (const spaceId of added) {
      await fetch('/api/admin/rag', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ spaceBrain: { spaceId, collectionId: col.id } }) })
    }
    for (const spaceId of removed) {
      await fetch('/api/admin/rag', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ spaceBrain: { spaceId, collectionId: null } }) })
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
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
</script>

<div class="rounded-md border border-line p-3">
  <div class="flex items-center gap-2">
    <span class="text-sm font-medium text-fg">{col.name}</span>
    <span class="rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{col.kind}</span>
    {#if col.auto}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">auto</span>{/if}
    <span class="ml-auto"></span>
    {#if !col.auto}
      <button type="button" onclick={() => void del()} class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger">
        Delete
      </button>
    {/if}
  </div>
  {#if col.description}<div class="mt-0.5 text-xs text-muted">{col.description}</div>{/if}
  <!-- Auto collections have fixed access (activity = your own scope; org-kb =
       everyone). Custom collections are bound + curated here. -->
  {#if !col.auto}
    <div class="mt-2.5 space-y-2">
      <label class="flex items-center gap-1.5 text-xs text-muted">
        <input type="checkbox" checked={bindingsAll} onchange={(e) => applyBindings({ all: e.currentTarget.checked })} class="accent-accent" />
        Everyone
      </label>
      {#if !bindingsAll && pickersPending}
        <div class="flex flex-col gap-2 sm:flex-row">
          {#each [0, 1, 2] as i (i)}
            <Skeleton class="h-8 min-w-0 flex-1" delay={i * 0.12} />
          {/each}
        </div>
      {/if}
      {#if !bindingsAll && !pickersPending}
        <div class="flex flex-col gap-2 sm:flex-row">
          <Combobox
            options={teams.map((t) => ({ value: t.id, label: t.name }))}
            selected={boundTeams}
            onChange={(ids) => applyBindings({ teams: ids })}
            multiple
            size="sm"
            placeholder="Bind teams"
            class="min-w-0 flex-1"
          />
          <Combobox
            options={users.map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id }))}
            selected={boundUsers}
            onChange={(ids) => applyBindings({ users: ids })}
            multiple
            size="sm"
            placeholder="Bind users"
            class="min-w-0 flex-1"
          />
          <Combobox
            options={(fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label }))}
            selected={boundAgents}
            onChange={(ids) => applyBindings({ agents: ids })}
            multiple
            size="sm"
            placeholder="Bind agents"
            class="min-w-0 flex-1"
          />
        </div>
      {/if}
      {#if !bindingsAll && (teamsList.notice || usersList.notice)}
        <div class="space-y-1">
          {#if teamsList.notice}<QueryError {...teamsList.notice} />{/if}
          {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
        </div>
      {/if}
      <div class="flex items-center gap-2">
        <span class="shrink-0 text-xs text-muted">Fed by KB spaces:</span>
        <Combobox
          options={spaces.map((s) => ({ value: s.id, label: s.name }))}
          selected={feedingSpaces}
          onChange={(ids) => void setSpaces(ids)}
          multiple
          size="sm"
          placeholder="None. Bind spaces to curate this brain"
          class="min-w-0 flex-1"
        />
      </div>
    </div>
  {/if}
</div>
