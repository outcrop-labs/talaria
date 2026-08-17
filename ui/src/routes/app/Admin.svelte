<script lang="ts">
  import { navigate, route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import RetrievalPanel from '@/components/admin/RetrievalPanel.svelte'
  import StoragePanel from '@/components/admin/StoragePanel.svelte'
  import SecretsPanel from '@/components/admin/SecretsPanel.svelte'
  import WorkspaceSecretsPanel from '@/components/admin/WorkspaceSecretsPanel.svelte'
  import { useAgents } from '@/lib/agents'
  import { useSession } from '@/lib/session'
  import { relativeTime } from '@/lib/fleet'
  import { fly, slide, staggerIn } from '@/lib/motion'
  import { GATEABLE_VIEWS, MANAGE_VIEWS } from '@/lib/nav'
  import { useEnabledApps } from '@/lib/apps'
  import AdminEmailPanel from './AdminEmailPanel.svelte'
  import AdminEncryptionPanel from './AdminEncryptionPanel.svelte'
  import AdminGithubPanel from './AdminGithubPanel.svelte'
  import AdminGuardrailsPanel from './AdminGuardrailsPanel.svelte'
  import AdminInstanceDomainPanel from './AdminInstanceDomainPanel.svelte'
  import AdminInvitesPanel from './AdminInvitesPanel.svelte'
  import AdminJudgePanel from './AdminJudgePanel.svelte'
  import AdminMemberDefaultsPanel from './AdminMemberDefaultsPanel.svelte'
  import AdminOrgGooglePanel from './AdminOrgGooglePanel.svelte'
  import AdminOrgPanel from './AdminOrgPanel.svelte'
  import AdminOutreachPanel from './AdminOutreachPanel.svelte'
  import AdminSettingsPanel from './AdminSettingsPanel.svelte'
  import AdminSignupDomainsPanel from './AdminSignupDomainsPanel.svelte'
  import AdminUserPermChips from './AdminUserPermChips.svelte'
  import { ADMIN_TABS, useAdminPermissions, useAdminUsers, type AdminTab } from './admin'

  // The admin console: people, their roles, and which agents each may use.
  const qc = useQueryClient()
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
  const appViews = $derived([
    ...enabledApps.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work! })),
    ...enabledApps.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage! })),
  ])
  const workViews = GATEABLE_VIEWS
  const manageViews = $derived([...MANAGE_VIEWS.map((v) => ({ ...v, sub: 'manage' })), ...appViews.map((v) => ({ ...v, sub: 'app' }))])
  let error = $state<string | null>(null)

  const update = async (userId: string, patch: { role?: 'admin' | 'member'; agentModels?: string[]; canMintKeys?: boolean; deniedViews?: string[]; allowedManageViews?: string[]; assistantElevated?: boolean }) => {
    error = null
    const r = await fetch('/api/admin/users', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ userId, ...patch }),
    })
    if (!r.ok) error = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'update failed'
    await qc.invalidateQueries({ queryKey: ['admin-users'] })
  }

  // /admin/security deep-links a concern.
  // THE URL IS THE TAB — /admin and /admin/<tab>.
  const tab = $derived(tabFromPath(route.pathname, '/admin', ADMIN_TABS.map((t) => t.id), 'org'))
  const setTab = (t: AdminTab) => {
    if (t === 'org') void navigate('/admin')
    else void navigate('/admin/:tab', { params: { tab: t } })
  }
</script>

{#if me && me.role !== 'admin'}
  <EmptyState icon="⛨" title="Admins only" hint="Ask an admin if you need access here." />
{:else}
  <div class="h-full overflow-y-auto p-8">
    <div class="mx-auto max-w-4xl space-y-6">
      <h1 class="text-2xl font-semibold tracking-tight text-fg">Admin</h1>

      <!-- One concern per tab; every panel keeps its own component. -->
      <Tabs items={ADMIN_TABS} value={tab} onChange={setTab} />

      <!-- Tab-pane grammar: pane rises in on switch (no exit), panels stagger —
           these are section stacks, one Panel per concern. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-6">
      {#if tab === 'org'}
        <AdminOrgPanel />
        <AdminInstanceDomainPanel />
        <AdminSignupDomainsPanel />
        <AdminEmailPanel />
        <AdminGithubPanel />
        <AdminOrgGooglePanel />
      {/if}
      {#if tab === 'agents'}
        <AdminJudgePanel />
        <AdminGuardrailsPanel />
        <AdminOutreachPanel />
      {/if}
      {#if tab === 'retrieval'}<RetrievalPanel />{/if}
      {#if tab === 'storage'}<StoragePanel />{/if}
      <!-- TWO DIFFERENT NOUNS ON ONE TAB, in the order an operator needs them: the
           inventory answers "is this instance healthy", the panel below answers
           "what may my agents spend". Sharing a tab is right — they are both
           credentials — and sharing a heading would not be. -->
      {#if tab === 'secrets'}<SecretsPanel />{/if}
      {#if tab === 'secrets'}<WorkspaceSecretsPanel />{/if}
      {#if tab === 'security'}
        <AdminEncryptionPanel />
        <AdminSettingsPanel />
      {/if}
      {#if tab === 'people'}<AdminInvitesPanel />{/if}
      {#if tab === 'people'}
        {#if perms}
          <AdminMemberDefaultsPanel {perms} />
        {:else if permsQuery.isError}
          <!-- Silence here reads as "no permission model configured". Say so. -->
          <Panel>
            <QueryError
              variant="compact"
              error={permsQuery.error}
              title="Could not load member permissions"
              onRetry={() => void permsQuery.refetch()}
            />
          </Panel>
        {/if}
      {/if}
      {#if tab === 'people'}
        <Panel>
          <SectionHeader
            class="mb-4"
            title="People"
            info="Roles, per-person agent access, and which views each member can reach. Empty = all (open by default); pick any to restrict. Admins always have full access."
          />
          {#if error}
            <div transition:slide={{ duration: 150 }} class="mb-2 text-xs text-danger">
              {error}
            </div>
          {/if}
          {#if usersQuery.isPending}
            <!-- Person-row skeletons: avatar + name/email bars + the role/keys
                 controls, so the resolved list lands without a jump. -->
            <div class="divide-y divide-line-subtle">
              {#each Array.from({ length: 4 }, (_, i) => i) as i (i)}
                <div class="flex items-center gap-3 py-3">
                  <Skeleton class="h-7 w-7 shrink-0 rounded-full" delay={i * 0.12} />
                  <div class="min-w-0 flex-1 space-y-1.5">
                    <Skeleton class="h-2.5 w-36 rounded-full" delay={i * 0.12} />
                    <Skeleton class="h-2 w-48 rounded-full" delay={i * 0.12 + 0.06} />
                  </div>
                  <Skeleton class="h-7 w-28 shrink-0" delay={i * 0.12} />
                  <Skeleton class="h-4 w-12 shrink-0" delay={i * 0.12} />
                  <Skeleton class="h-2.5 w-16 shrink-0 rounded-full" delay={i * 0.12} />
                </div>
              {/each}
            </div>
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
            <ul class="divide-y divide-line-subtle">
              {#each users as u (u.id)}
                <!-- One ALLOW list spanning both worlds: work views default in
                     (denials stored), Manage views default out (allows stored). -->
                {@const allowedViews = [
                  ...workViews.filter((v) => !u.deniedViews.includes(v.to)).map((v) => v.to),
                  ...manageViews.filter((v) => u.allowedManageViews.includes(v.to)).map((v) => v.to),
                ]}
                <li class="-mx-2 space-y-2 rounded-md px-2 py-3 transition-colors hover:bg-hover">
                  <div class="flex items-center gap-3">
                    <Avatar name={u.name ?? u.email} class="h-7 w-7" />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-sm text-fg">{u.name ?? u.email ?? u.id}</span>
                      <span class="block truncate text-xs text-muted">
                        {u.name && u.email ? u.email : `seen ${relativeTime(u.lastSeenAt)}`}
                      </span>
                    </span>
                    <Select
                      value={u.role}
                      size="sm"
                      disabled={u.pinnedAdmin || u.id === me?.id}
                      title={u.pinnedAdmin ? 'Pinned admin via AUTH_ADMIN_EMAILS' : u.id === me?.id ? 'You cannot demote yourself' : undefined}
                      onchange={(e) => void update(u.id, { role: e.currentTarget.value as 'admin' | 'member' })}
                      class="w-28 shrink-0"
                    >
                      <option value="member">Member</option>
                      <option value="admin">Admin</option>
                    </Select>
                    {#if u.role === 'admin' && u.assistantModel}
                      <Checkbox
                        class="shrink-0"
                        title={`Give ${u.assistantModel} org-wide view/edit: every board, every channel and relay (never DMs), and editor rights on all org-visible knowledge and artifacts. Only while this user is an admin.`}
                        checked={u.assistantElevated}
                        onChange={(checked) => void update(u.id, { assistantElevated: checked })}
                        label="elevated assistant"
                      />
                    {/if}
                    <span class="w-16 shrink-0 text-right font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(u.lastSeenAt)}</span>
                  </div>
                  {#if u.role !== 'admin'}
                    <div class="flex items-center gap-2 pl-10">
                      <Combobox
                        options={agentOptions}
                        selected={u.agentModels}
                        onChange={(models) => void update(u.id, { agentModels: models })}
                        multiple
                        size="sm"
                        placeholder="All agents"
                        class="min-w-0 flex-1"
                      />
                      <Combobox
                        options={[
                          ...workViews.map((v) => ({ value: v.to, label: v.label, sub: 'work' })),
                          ...manageViews.map((v) => ({ value: v.to, label: v.label, sub: v.sub })),
                        ]}
                        selected={allowedViews}
                        onChange={(views) =>
                          void update(u.id, {
                            deniedViews: workViews.filter((v) => !views.includes(v.to)).map((v) => v.to),
                            allowedManageViews: manageViews.filter((v) => views.includes(v.to)).map((v) => v.to),
                          })}
                        multiple
                        size="sm"
                        disabled={appsList.failed}
                        placeholder="Work views"
                        class="min-w-0 flex-1"
                      />
                      <!-- ^ Editing writes the whole allow-list back. While the
                           app views are missing, that write would drop them. -->
                    </div>
                    {#if appsList.notice}<div class="pl-10"><QueryError {...appsList.notice} /></div>{/if}
                    {#if perms}<AdminUserPermChips userId={u.id} {perms} />{/if}
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </Panel>
      {/if}
      </div>
      {/key}
    </div>
  </div>
{/if}
