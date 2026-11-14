<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useAgents } from '@/lib/agents'
  import { useEnabledApps } from '@/lib/apps'
  import { appGrantViewsOf } from '@/lib/app-grants'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { GATEABLE_VIEWS, MANAGE_VIEWS } from '@/lib/nav'
  import { useSession } from '@/lib/session'
  import AdminUserPermChips from './AdminUserPermChips.svelte'
  import { useAdminPermissions, useAdminUsers, type AdminUser } from './admin'

  // The people of the instance as a table: who they are, their role, a one-line
  // summary of what they can reach, and the full access controls under the row.
  const qc = useQueryClient()
  const autosave = useAutosave()
  const session = useSession()
  const me = $derived(session.data)
  const usersQuery = useAdminUsers()
  const users = $derived(usersQuery.data)
  const permsQuery = useAdminPermissions()
  const perms = $derived(permsQuery.data)
  const fleetQuery = useAgents()
  const agentOptions = $derived((fleetQuery.data?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))
  // Enabled apps join the per-person view checklist as EXPLICIT grants —
  // every app view (work and manage) defaults denied for members; an admin
  // adds each one per person, same storage as core Manage views.
  // This one is not cosmetic. `allowedManageViews` is written back WHOLESALE
  // from `manageViews` below, so with the app list defaulted to `[]` a failed
  // /api/apps meant that toggling any unrelated view silently REVOKED every
  // app manage grant the person had — a permissions write derived from a read
  // that never happened.
  const appsList = listQuery(useEnabledApps(), { title: 'Could not load installed apps', variant: 'inline' })
  const enabledApps = $derived(appsList.rows)
  // Each surface is its own grant; a settings tab rides the manage grant, so
  // a settings-only app is offered too (app-grants.ts). Each option is named
  // for the app and what it opens ("CRM · work", "CRM · settings"): the
  // surfaces' own labels are often the same word.
  const appViews = $derived(appGrantViewsOf(enabledApps))
  const workViews = GATEABLE_VIEWS
  const manageViews = $derived([
    ...MANAGE_VIEWS.map((v) => ({ ...v, sub: 'manage' })),
    ...appViews.map((v) => ({ to: v.to, label: v.title, sub: 'app' })),
  ])
  // The last admin cannot be demoted — not even by themselves (the server
  // enforces it too; this only keeps the control from offering it).
  const adminsRemaining = $derived((users ?? []).filter((u) => u.role === 'admin').length)
  let error = $state<string | null>(null)
  // One person's access detail open at a time.
  let editing = $state<string | null>(null)

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[]; canMintKeys?: boolean; deniedViews?: string[]; allowedManageViews?: string[]; assistantElevated?: boolean }) => {
    error = null
    try {
      await putJson<{ ok: true }>('/api/admin/users', { userId, ...patch })
      // Whatever changed for a person, their own row says so.
      autosave.flash(userId)
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['admin-users'] })
  }

  // One ALLOW list spanning both worlds: work views default in (denials
  // stored), Manage views default out (allows stored).
  const allowedViewsOf = (u: AdminUser) => [
    ...workViews.filter((v) => !u.deniedViews.includes(v.to)).map((v) => v.to),
    ...manageViews.filter((v) => u.allowedManageViews.includes(v.to)).map((v) => v.to),
  ]

  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`
  const accessSummary = (u: AdminUser): string => {
    if (u.role === 'admin') return u.assistantElevated ? 'Everything, and so can their assistant' : 'Everything'
    const parts = [
      u.agentModels.length ? count(u.agentModels.length, 'agent', 'agents') : 'All agents',
      count(allowedViewsOf(u).length, 'page', 'pages'),
    ]
    if (perms) {
      const overrides = perms.overrides[u.id] ?? {}
      const allowed = perms.catalog.filter((p) => overrides[p.id] ?? perms.orgDefaults[p.id] ?? p.memberDefault).length
      parts.push(count(allowed, 'permission', 'permissions'))
    }
    return parts.join(' · ')
  }
  const editable = (u: AdminUser) => u.role !== 'admin' || !!u.assistantModel

  const nameOf = (u: AdminUser) => u.name ?? u.email ?? u.id
  const toggle = (id: string) => (editing = editing === id ? null : id)

  // The header's captions and each row's cells share these widths, so the
  // columns line up without a table.
  const ROLE = 'w-[7.5rem] shrink-0'
  const ACCESS = 'w-[15rem] shrink-0'
  const SEEN = 'w-[4.5rem] shrink-0'
</script>

<SettingsSection
  title="People"
  info="Admins can do everything. For a member, pick which agents and pages they can reach, and change any permission that should differ from the member defaults below."
>
  <SettingsGroup>
    <SettingsHeaderRow label="Name" menu>
      <span class={ROLE}>Role</span>
      <span class={ACCESS}>Access</span>
      <span class={SEEN}>Last seen</span>
    </SettingsHeaderRow>
    {#if error}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
    {/if}
    {#if usersQuery.isPending}
      <div class="px-5 py-4"><SkeletonRows rows={4} avatar /></div>
    {:else if !users}
      <!-- The most dangerous empty render on the whole surface: an admin
           seeing no rows concludes the org is empty (or that a person they
           just off-boarded is gone) when the read simply failed. -->
      <QueryError
        variant="compact"
        error={usersQuery.error}
        title="Could not load your people"
        onRetry={() => void usersQuery.refetch()}
      />
    {:else}
      {#each users as u (u.id)}
        {@const lastAdmin = u.role === 'admin' && adminsRemaining === 1}
        {@const open = editing === u.id && editable(u)}
        <div>
          <SettingsRow
            hint={u.name && u.email ? u.email : undefined}
            onOpen={editable(u) ? () => toggle(u.id) : undefined}
            openLabel={`Edit access for ${nameOf(u)}`}
            saved={autosave.saved(u.id)}
            class="min-h-[58px] py-2"
          >
            {#snippet lead()}<Avatar name={u.name ?? u.email} class="h-8 w-8" />{/snippet}
            {#snippet label()}{nameOf(u)}{/snippet}
            {#snippet meta()}
              {#if u.id === me?.id}<Chip>You</Chip>{/if}
              {#if u.hasPasswordAccount}
                <Chip title="Can also sign in with an email and password. Manage it under Password sign-in below.">Password</Chip>
              {/if}
            {/snippet}
            <Select
              value={u.role}
              size="sm"
              aria-label={`Role for ${nameOf(u)}`}
              disabled={lastAdmin}
              title={lastAdmin ? 'Cannot demote the last admin' : undefined}
              onchange={(e) => void update(u.id, { role: e.currentTarget.value as 'admin' | 'member' })}
              class={ROLE}
            >
              <option value="member">Member</option>
              <option value="admin">Admin</option>
            </Select>
            <span class="{ACCESS} truncate font-sans text-[13px] leading-5 text-muted" title={accessSummary(u)}>
              {accessSummary(u)}
            </span>
            <span class="{SEEN} font-sans text-[13px] leading-5 tabular-nums text-muted" title={new Date(u.lastSeenAt).toLocaleString()}>
              {relativeTime(u.lastSeenAt)}
            </span>
            <SettingsRowMenu
              label={`Actions for ${nameOf(u)}`}
              items={editable(u) ? [{ label: open ? 'Close access' : 'Edit access', onSelect: () => toggle(u.id) }] : []}
            />
          </SettingsRow>
          {#if open}
            <div transition:slide={{ duration: 150 }}>
              <div class="space-y-5 border-t border-line-subtle py-4 pl-16 pr-5">
                {#if u.role === 'admin'}
                  <div class="flex items-center gap-6">
                    <div class="min-w-0 flex-1">
                      <div class="font-sans text-sm font-medium leading-5 text-fg">Assistant can see everything</div>
                      <div class="font-sans text-[13px] leading-5 text-muted">
                        Lets <span class="font-mono text-xs">{u.assistantModel}</span> read and edit every board, channel and shared document, but never direct messages.
                      </div>
                    </div>
                    <Toggle
                      title="Assistant can see everything"
                      checked={u.assistantElevated}
                      onChange={(checked) => void update(u.id, { assistantElevated: checked })}
                    />
                  </div>
                {:else}
                  <div class="grid gap-5 sm:grid-cols-2">
                    <SettingsField label="Agents" hint="Leave empty to allow every agent.">
                      <Combobox
                        options={agentOptions}
                        selected={u.agentModels}
                        onChange={(models) => void update(u.id, { agentModels: models })}
                        multiple
                        size="sm"
                        placeholder="All agents"
                        class="w-full"
                      />
                    </SettingsField>
                    <SettingsField label="Pages" hint="The pages this person can open.">
                      <!-- Editing writes the whole allow-list back. While the
                           app views are missing, that write would drop them. -->
                      <Combobox
                        options={[
                          ...workViews.map((v) => ({ value: v.to, label: v.label, sub: 'work' })),
                          ...manageViews.map((v) => ({ value: v.to, label: v.label, sub: v.sub })),
                        ]}
                        selected={allowedViewsOf(u)}
                        onChange={(views) =>
                          void update(u.id, {
                            deniedViews: workViews.filter((v) => !views.includes(v.to)).map((v) => v.to),
                            allowedManageViews: manageViews.filter((v) => views.includes(v.to)).map((v) => v.to),
                          })}
                        multiple
                        size="sm"
                        disabled={appsList.failed}
                        placeholder="No pages"
                        class="w-full"
                      />
                    </SettingsField>
                  </div>
                  {#if appsList.notice}<QueryError {...appsList.notice} />{/if}
                  {#if perms}
                    <SettingsField label="Permissions" hint="A dot marks a change from the member defaults, and selecting it again goes back.">
                      <AdminUserPermChips userId={u.id} {perms} onSaved={() => autosave.flash(u.id)} />
                    </SettingsField>
                  {/if}
                {/if}
              </div>
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </SettingsGroup>
</SettingsSection>
