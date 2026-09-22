<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import TeamMembers from '@/components/board/TeamMembers.svelte'
  import { useAgents } from '@/lib/agents'
  import { GATEABLE_VIEWS, MANAGE_VIEWS } from '@/lib/nav'
  import { useSession } from '@/lib/session'
  import { toastError } from '@/lib/toast.svelte'
  import {
    addTeamAgent,
    patchTeam,
    putTeamAccess,
    removeTeamAgent,
    useTeamAccess,
    useTeamAgents,
    type Team,
  } from '@/lib/teams'
  import AdminPermChip from './AdminPermChip.svelte'
  import { permGroups, useAdminPermissions, type PermCatalogEntry } from './admin'

  let { team, onChanged }: { team: Team; onChanged: () => void } = $props()

  const qc = useQueryClient()
  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const isAdmin = $derived(session?.role === 'admin')
  const canManage = $derived(team.role === 'owner')

  let name = $state(team.name)
  let description = $state(team.description ?? '')

  const agentsList = listQuery(useTeamAgents(() => team.id), {
    title: 'Could not load this team’s agents',
    variant: 'compact',
  })
  const agents = $derived(agentsList.rows)
  const accessQuery = useTeamAccess(
    () => team.id,
    () => session?.role === 'admin',
  )
  const permsQuery = useAdminPermissions(() => session?.role === 'admin')
  const catalog = $derived(permsQuery.data?.catalog ?? [])
  const orgDefaults = $derived(permsQuery.data?.orgDefaults ?? {})
  const orgDefaultOf = (p: PermCatalogEntry) => orgDefaults[p.id] ?? p.memberDefault
  const fleetQuery = useAgents()
  const fleetFailed = $derived(fleetQuery.isError && fleetQuery.data === undefined)
  const agentOptions = $derived(
    (fleetQuery.data?.agents ?? [])
      .filter((a) => !agents.some((x) => x.agentModel === a.id))
      .map((a) => ({ value: a.id, label: a.label, sub: a.role })),
  )

  const refreshAgents = () => {
    void qc.invalidateQueries({ queryKey: ['team-agents', team.id] })
    onChanged()
  }

  const saveMeta = async () => {
    if (!canManage) return
    const nextName = name.trim()
    const nextDesc = description.trim()
    const prevDesc = (team.description ?? '').trim()
    if (!nextName) {
      name = team.name
      return
    }
    const patch: { name?: string; description?: string | null } = {}
    if (nextName !== team.name) patch.name = nextName
    if (nextDesc !== prevDesc) patch.description = nextDesc || null
    if (!patch.name && patch.description === undefined) return
    try {
      await patchTeam(team.id, patch)
    } catch (e) {
      toastError('Save failed', e)
      name = team.name
      description = team.description ?? ''
      return
    }
    onChanged()
  }

  const addAgent = async (model: string) => {
    try {
      await addTeamAgent(team.id, model)
    } catch (e) {
      toastError('Could not add that agent', e)
      return
    }
    refreshAgents()
  }

  const dropAgent = async (model: string) => {
    try {
      await removeTeamAgent(team.id, model)
    } catch (e) {
      toastError('Remove failed', e)
      return
    }
    refreshAgents()
  }

  const saveAccess = async (body: {
    deniedViews?: string[]
    allowedManageViews?: string[]
    permissions?: Record<string, boolean | null>
  }) => {
    try {
      await putTeamAccess(team.id, body)
    } catch (e) {
      toastError('Could not save access', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['team-access', team.id] })
  }

  const toggleWork = (to: string) => {
    const current = accessQuery.data
    if (!current) return
    const denied = new Set(current.deniedViews)
    if (denied.has(to)) denied.delete(to)
    else denied.add(to)
    void saveAccess({ deniedViews: [...denied] })
  }

  const toggleManage = (to: string) => {
    const current = accessQuery.data
    if (!current) return
    const allowed = new Set(current.allowedManageViews)
    if (allowed.has(to)) allowed.delete(to)
    else allowed.add(to)
    void saveAccess({ allowedManageViews: [...allowed] })
  }

  const togglePerm = (p: PermCatalogEntry) => {
    const current = accessQuery.data
    if (!current) return
    const inherit = orgDefaultOf(p)
    const stored = current.permissions[p.id]
    const effective = stored ?? inherit
    const next = !effective
    void saveAccess({ permissions: { [p.id]: next === inherit ? null : next } })
  }
</script>

<div class="min-h-0 flex-1 overflow-y-auto p-6">
  <div class="space-y-6">
    <div class="space-y-2">
      <Input
        bind:value={name}
        disabled={!canManage}
        placeholder="Name"
        onblur={() => void saveMeta()}
      />
      <Textarea
        bind:value={description}
        disabled={!canManage}
        rows={3}
        placeholder="Description"
        onblur={() => void saveMeta()}
      />
    </div>

    <div>
      <SectionHeader title="People" info="Humans on this team. Owners govern the roster, name, and agents." />
      <TeamMembers teamId={team.id} canManage={canManage} />
    </div>

    <div>
      <SectionHeader title="Agents" info="Agents on this team act as members of it. They never own a team." />
      {#if canManage}
        {#if fleetFailed}
          <QueryError
            variant="compact"
            error={fleetQuery.error}
            title="Could not load agents"
            onRetry={() => void fleetQuery.refetch()}
          />
        {:else}
          <Combobox
            options={agentOptions}
            selected={[]}
            onChange={(vals) => {
              const model = vals[0]
              if (model) void addAgent(model)
            }}
            placeholder="Add agent"
            size="sm"
          />
        {/if}
      {/if}
      {#if agentsList.notice}<div class="mt-3"><QueryError {...agentsList.notice} /></div>{/if}
      <ul class="mt-3 space-y-1.5">
        {#each agents as a (a.agentModel)}
          <li class="flex items-center gap-2 rounded-md px-1 py-2">
            <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{a.label ?? a.agentModel}</span>
            {#if canManage}
              <Button variant="ghost" size="xs" class="hover:text-danger" onclick={() => void dropAgent(a.agentModel)}>
                Remove
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
    </div>

    {#if isAdmin}
      <div>
        <SectionHeader
          title="Access"
          info="Work-view denials and manage-view grants for everyone on this team. Per-user overrides still win."
        />
        {#if accessQuery.isError && accessQuery.data === undefined}
          <QueryError
            variant="compact"
            error={accessQuery.error}
            title="Could not load team access"
            onRetry={() => void accessQuery.refetch()}
          />
        {:else if accessQuery.data && catalog.length}
          {@const access = accessQuery.data}
          <div class="space-y-3">
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="w-16 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Work</span>
              {#each GATEABLE_VIEWS as v (v.to)}
                {@const denied = access.deniedViews.includes(v.to)}
                <AdminPermChip
                  entry={{ id: v.to, label: v.label, hint: 'Work view. Filled means members of this team may open it.', group: 'Work', memberDefault: true }}
                  effective={!denied}
                  overridden={denied}
                  onToggle={() => toggleWork(v.to)}
                />
              {/each}
            </div>
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="w-16 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Manage</span>
              {#each MANAGE_VIEWS as v (v.to)}
                {@const granted = access.allowedManageViews.includes(v.to)}
                <AdminPermChip
                  entry={{ id: v.to, label: v.label, hint: 'Manage view. Filled means members of this team have been granted it.', group: 'Manage', memberDefault: false }}
                  effective={granted}
                  overridden={granted}
                  onToggle={() => toggleManage(v.to)}
                />
              {/each}
            </div>
            {#each permGroups(catalog) as [group, entries] (group)}
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="w-16 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{group}</span>
                {#each entries as p (p.id)}
                  {@const stored = access.permissions[p.id]}
                  {@const inherit = orgDefaultOf(p)}
                  <AdminPermChip
                    entry={p}
                    effective={stored ?? inherit}
                    overridden={stored !== undefined}
                    onToggle={() => togglePerm(p)}
                  />
                {/each}
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>
