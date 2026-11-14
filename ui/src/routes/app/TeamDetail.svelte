<script lang="ts" module>
  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`

  /** A team's size in words, for its picker row and its title. */
  export const teamCounts = (t: { memberCount: number; agentCount: number }): string =>
    `${count(t.memberCount, 'person', 'people')} · ${count(t.agentCount, 'agent', 'agents')}`
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useAgents } from '@/lib/agents'
  import { errorMessage } from '@/lib/fetch-json'
  import { GATEABLE_VIEWS, MANAGE_VIEWS } from '@/lib/nav'
  import { useEnabledApps } from '@/lib/apps'
  import { appGrantViewsOf } from '@/lib/app-grants'
  import { useSession } from '@/lib/session'
  import { toastError } from '@/lib/toast.svelte'
  import {
    addTeamAgent,
    addTeamMember,
    patchTeam,
    putTeamAccess,
    removeTeamAgent,
    removeTeamMember,
    useTeamAccess,
    useTeamAgents,
    useTeamMembers,
    type Team,
    type TeamRole,
  } from '@/lib/teams'
  import { permGroups, useAdminPermissions, type PermCatalogEntry } from './admin'

  let {
    team,
    onChanged,
    onDelete,
  }: {
    team: Team
    onChanged: () => void
    /** Deletes the team. The confirmation is the caller's. Owners only. */
    onDelete: () => void
  } = $props()

  const qc = useQueryClient()
  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const isAdmin = $derived(session?.role === 'admin')
  const canManage = $derived(team.role === 'owner')
  // The name and the description save on change for an owner, and the access
  // switches for an admin, who need not own the team.
  const autosave = useAutosave(() => canManage || isAdmin)

  // svelte-ignore state_referenced_locally -- reason: per-team drafts; the view remounts per team selection
  let name = $state(team.name)
  // svelte-ignore state_referenced_locally -- reason: per-team drafts; the view remounts per team selection
  let description = $state(team.description ?? '')

  // A failed member read must not render as an empty roster next to a live
  // "Add": the fix people reach for is re-adding people who are already there.
  const membersList = listQuery(useTeamMembers(() => team.id), {
    title: 'Could not load this team’s members',
    variant: 'compact',
  })
  const members = $derived(membersList.rows)
  const ownerCount = $derived(members.filter((m) => m.role === 'owner').length)
  const canRemove = (role: TeamRole) => canManage && (role !== 'owner' || ownerCount > 1)
  let newRole = $state<TeamRole>('member')
  let memberError = $state<string | null>(null)
  const refreshMembers = () => qc.invalidateQueries({ queryKey: ['team-members', team.id] })

  const addMember = async (email: string) => {
    memberError = null
    try {
      await addTeamMember(team.id, email, newRole)
    } catch (e) {
      memberError = errorMessage(e)
      return
    }
    void refreshMembers()
    void qc.invalidateQueries({ queryKey: ['teams'] })
  }

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
  // Enabled apps' views, grantable to a team the same way as to a person —
  // each surface its own grant (app-grants.ts). A failed read only hides the
  // row: the toggles below edit the STORED set, never a list derived from it.
  const appsQuery = useEnabledApps()
  const appViews = $derived(appGrantViewsOf(appsQuery.data ?? []))
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
    autosave.flash()
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
    autosave.flash()
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

  /** One switch of the Access section. */
  interface Grant {
    id: string
    label: string
    hint: string
    on: boolean
    /** A permission set away from the workspace default. */
    changed: boolean
    toggle: () => void
  }
  const grantGroups = $derived.by((): { label: string; grants: Grant[] }[] => {
    const access = accessQuery.data
    if (!access) return []
    const granted = (to: string) => access.allowedManageViews.includes(to)
    return [
      {
        label: 'Work views',
        grants: GATEABLE_VIEWS.map((v) => ({
          id: v.to,
          label: v.label,
          hint: 'Work view. On means members of this team may open it.',
          on: !access.deniedViews.includes(v.to),
          changed: false,
          toggle: () => toggleWork(v.to),
        })),
      },
      {
        label: 'Manage views',
        grants: MANAGE_VIEWS.map((v) => ({
          id: v.to,
          label: v.label,
          hint: 'Manage view. On means members of this team have been granted it.',
          on: granted(v.to),
          changed: false,
          toggle: () => toggleManage(v.to),
        })),
      },
      {
        label: 'Apps',
        grants: appViews.map((v) => ({
          id: v.to,
          label: v.title,
          hint: `${v.to}. On means members of this team have been granted it.`,
          on: granted(v.to),
          changed: false,
          toggle: () => toggleManage(v.to),
        })),
      },
      ...permGroups(catalog).map(([group, entries]) => ({
        label: `${group} permissions`,
        grants: entries.map((p) => {
          const stored = access.permissions[p.id]
          return {
            id: p.id,
            label: p.label,
            hint: p.hint,
            on: stored ?? orgDefaultOf(p),
            changed: stored !== undefined,
            toggle: () => togglePerm(p),
          }
        }),
      })),
    ].filter((g) => g.grants.length > 0)
  })

  const ROLE_LABEL: Record<TeamRole, string> = { owner: 'Owner', member: 'Member' }
  // The People list's role column: the caption and every cell take this width.
  const ROLE = 'w-20 shrink-0'
  const subtitle = $derived(
    team.role ? `${teamCounts(team)} · You are ${team.role === 'owner' ? 'an owner' : 'a member'}` : teamCounts(team),
  )

</script>

<div class="flex h-full flex-col">
<div class="min-h-0 flex-1 overflow-y-auto p-5">
  <header>
    <h2 class="break-words font-sans text-lg font-semibold leading-6 text-fg">{team.name}</h2>
    <p class="mt-1 font-sans text-[13px] leading-5 text-muted">
      {subtitle}
    </p>
  </header>

  <div class="mt-6 space-y-8">
    <SettingsSection title="Details">
      <SettingsGroup bare>
        <SettingsRow label="Name" as="label">
          <Input size="sm" class="w-[280px]" bind:value={name} disabled={!canManage} placeholder="Name" onblur={() => void saveMeta()} />
        </SettingsRow>
        <SettingsRow label="Description" stacked>
          <Textarea
            bind:value={description}
            disabled={!canManage}
            rows={2}
            placeholder="What this team is responsible for"
            onblur={() => void saveMeta()}
          />
        </SettingsRow>
      </SettingsGroup>
    </SettingsSection>

    <SettingsSection title="People" description="Owners manage the team's name, people and agents.">
      {#if membersList.notice}<div class="mb-3"><QueryError {...membersList.notice} /></div>{/if}
      <SettingsGroup bare>
        {#if membersList.pending}
          <SkeletonRows rows={3} avatar class="py-4" />
        {:else if members.length > 0}
          <SettingsHeaderRow label="Name" menu><span class={ROLE}>Role</span></SettingsHeaderRow>
        {/if}
        {#each members as m (m.userId)}
          {@const person = m.name ?? m.email ?? m.userId}
          <SettingsRow label={person} wrap hint={m.name && m.email ? m.email : undefined}>
            {#snippet lead()}<Avatar name={m.name ?? m.email} />{/snippet}
            <span class="{ROLE} font-sans text-[13px] leading-5 text-muted">{ROLE_LABEL[m.role]}</span>
            <SettingsRowMenu
              label={`Actions for ${m.name ?? m.email ?? 'this person'}`}
              items={canRemove(m.role)
                ? [{ label: 'Remove from team', danger: true, onSelect: () => void removeTeamMember(team.id, m.userId).then(refreshMembers) }]
                : []}
            />
          </SettingsRow>
        {/each}
        {#if canManage}
          <SettingsRow label="Add a person" apart>
            <UserPicker
              class="w-[280px]"
              size="sm"
              placeholder="Pick a person"
              exclude={members.map((m) => m.userId)}
              onPick={(u) => {
                if (u.email) void addMember(u.email)
              }}
            />
            <Select bind:value={newRole} size="sm" class="w-32" aria-label="Role for the person you add">
              <option value="member">Member</option>
              <option value="owner">Owner</option>
            </Select>
          </SettingsRow>
        {/if}
      </SettingsGroup>
      {#if memberError}<div class="mt-2 font-sans text-[13px] leading-5 text-danger">{memberError}</div>{/if}
    </SettingsSection>

    <SettingsSection title="Agents" description="Agents on a team act as its members and never own it.">
      {#if agentsList.notice}<div class="mb-3"><QueryError {...agentsList.notice} /></div>{/if}
      <SettingsGroup bare>
        {#each agents as a (a.agentModel)}
          <SettingsRow label={a.label ?? a.agentModel} wrap>
            {#snippet lead()}<Avatar name={a.label ?? a.agentModel} />{/snippet}
            {#if canManage}
              <SettingsRowMenu
                label={`Actions for ${a.label ?? a.agentModel}`}
                items={[{ label: 'Remove from team', danger: true, onSelect: () => void dropAgent(a.agentModel) }]}
              />
            {/if}
          </SettingsRow>
        {/each}
        {#if canManage}
          {#if fleetFailed}
            <div class="py-4">
              <QueryError
                variant="compact"
                error={fleetQuery.error}
                title="Could not load agents"
                onRetry={() => void fleetQuery.refetch()}
              />
            </div>
          {:else}
            <SettingsRow label="Add an agent" apart>
              <Combobox
                class="w-[280px]"
                options={agentOptions}
                selected={[]}
                onChange={(vals) => {
                  const model = vals[0]
                  if (model) void addAgent(model)
                }}
                placeholder="Pick an agent"
                size="sm"
              />
            </SettingsRow>
          {/if}
        {:else if agents.length === 0 && !agentsList.pending && !agentsList.notice}
          <SettingsRow label="No agents on this team" />
        {/if}
      </SettingsGroup>
    </SettingsSection>

    {#if isAdmin}
      <SettingsSection title="Access" description="What everyone on this team can open and do, unless a person's own settings say otherwise.">
        {#if accessQuery.isError && accessQuery.data === undefined}
          <QueryError
            variant="compact"
            error={accessQuery.error}
            title="Could not load team access"
            onRetry={() => void accessQuery.refetch()}
          />
        {:else if accessQuery.data && catalog.length}
          <SettingsGroup bare>
            {#each grantGroups as group (group.label)}
              <div class="py-4">
                <div class="font-sans text-sm font-medium leading-5 text-fg">{group.label}</div>
                <div class="mt-3 grid grid-cols-2 gap-x-6 gap-y-3 lg:grid-cols-3">
                  {#each group.grants as grant (grant.id)}
                    {#snippet grantLabel()}
                      <span class="min-w-0 truncate">{grant.label}</span>
                      {#if grant.changed}<StatusDot status="warn" title="Changed from the workspace default" />{/if}
                    {/snippet}
                    <Toggle
                      checked={grant.on}
                      onChange={grant.toggle}
                      title={grant.hint}
                      label={grantLabel}
                      class="min-w-0 gap-3 font-sans text-[13px] leading-5 text-fg"
                    />
                  {/each}
                </div>
              </div>
            {/each}
          </SettingsGroup>
          {#if grantGroups.some((g) => g.grants.some((grant) => grant.changed))}
            <p class="mt-3 flex items-center gap-2 font-sans text-xs leading-4 text-muted">
              <StatusDot status="warn" /> Changed from the workspace default
            </p>
          {/if}
        {/if}
      </SettingsSection>
    {/if}
  </div>
</div>
<!-- Where the other pick-and-edit pages put it: the pane's pinned foot, at its
     left, level with the picker's New team. -->
{#if canManage}
  <div class="flex h-[53px] shrink-0 items-center border-t border-line px-5">
    <DangerLink onClick={onDelete}>Delete team</DangerLink>
  </div>
{/if}
</div>
