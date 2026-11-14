<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { navigate, route } from '@/router'
  import { instanceAt, type InstanceTabId } from '@/lib/settings-nav'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RetrievalPanel from '@/components/admin/RetrievalPanel.svelte'
  import AgentWebSearchPanel from '@/components/admin/AgentWebSearchPanel.svelte'
  import SearchEndpointPanel from '@/components/admin/SearchEndpointPanel.svelte'
  import StoragePanel from '@/components/admin/StoragePanel.svelte'
  import SecretsPanel from '@/components/admin/SecretsPanel.svelte'
  import WorkspaceSecretsPanel from '@/components/admin/WorkspaceSecretsPanel.svelte'
  import { useSession } from '@/lib/session'
  import { fly, staggerIn } from '@/lib/motion'
  import AdminEmailPanel from './AdminEmailPanel.svelte'
  import AdminEncryptionPanel from './AdminEncryptionPanel.svelte'
  import AdminGitHostsPanel from './AdminGitHostsPanel.svelte'
  import AdminRepoCreationSection from './AdminRepoCreationSection.svelte'
  import AdminRepoFlowSection from './AdminRepoFlowSection.svelte'
  import AdminGoogleClientPanel from './AdminGoogleClientPanel.svelte'
  import AdminGuardrailsPanel from './AdminGuardrailsPanel.svelte'
  import AdminInstanceDomainPanel from './AdminInstanceDomainPanel.svelte'
  import AdminInvitesPanel from './AdminInvitesPanel.svelte'
  import AdminCodingAccountsPanel from './AdminCodingAccountsPanel.svelte'
  import AdminDecidePanel from './AdminDecidePanel.svelte'
  import AdminJudgePanel from './AdminJudgePanel.svelte'
  import AdminMemberDefaultsPanel from './AdminMemberDefaultsPanel.svelte'
  import AdminOrgGooglePanel from './AdminOrgGooglePanel.svelte'
  import AdminOrgPanel from './AdminOrgPanel.svelte'
  import AdminOutreachPanel from './AdminOutreachPanel.svelte'
  import AdminPasswordAccountsPanel from './AdminPasswordAccountsPanel.svelte'
  import AdminPeoplePanel from './AdminPeoplePanel.svelte'
  import AdminSettingsPanel from './AdminSettingsPanel.svelte'
  import AdminSignupDomainsPanel from './AdminSignupDomainsPanel.svelte'
  import AdminUpdatePanel from './AdminUpdatePanel.svelte'
  import { useAdminPermissions } from './admin'

  const session = useSession()
  const me = $derived(session.data)
  const permsQuery = useAdminPermissions()
  const perms = $derived(permsQuery.data)
  // Each tab's one primary action sits in the page header; the panel that
  // owns the dialog binds its open state here.
  let inviting = $state(false)
  let addingHost = $state(false)
  let addingCredentials = $state(false)

  // /admin/security deep-links a concern.
  // THE URL IS THE TAB — /admin and /admin/<tab>. Which of the Instance
  // group's pages this is follows from the tab: each page shows only its own.
  const here = $derived(instanceAt(route.pathname))
  const tab = $derived(here.tab)
  const setTab = (t: InstanceTabId) => {
    if (t === 'org') void navigate('/admin')
    else void navigate('/admin/:tab', { params: { tab: t } })
  }
</script>

{#if me && me.role !== 'admin'}
  <EmptyState icon="⛨" title="Admins only" hint="Ask an admin if you need access here." />
{:else}
  <!-- One concern per tab; every panel keeps its own component. -->
  <SettingsPage
    title={here.page.label}
    blurb={here.page.blurb}
    tabs={here.page.tabs}
    {tab}
    onTab={setTab}
  >
      {#snippet actions()}
        {#if tab === 'people'}
          <Button size="sm" onclick={() => (inviting = true)}>Invite people</Button>
        {:else if tab === 'git'}
          <Button size="sm" onclick={() => (addingHost = true)}>Add host</Button>
        {:else if tab === 'secrets'}
          <Button size="sm" onclick={() => (addingCredentials = true)}>Add credentials</Button>
        {/if}
      {/snippet}

      <!-- Tab-pane grammar: pane rises in on switch (no exit), panels stagger —
           these are section stacks, one Panel per concern. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-8">
      {#if tab === 'org'}
        <!-- The business agents are told about saves with a button, because
             saving it rewrites every agent. The instance's own name and
             address are the card under it. -->
        <SettingsSection title="Identity">
          <SettingsGroup><AdminOrgPanel /></SettingsGroup>
        </SettingsSection>
        <AdminInstanceDomainPanel />
        <AdminSignupDomainsPanel />
        <AdminEmailPanel />
        <AdminGoogleClientPanel />
        <AdminOrgGooglePanel />
      {/if}
      {#if tab === 'agents'}
        <!-- The decision-model port leads the tab because the panels under it
             are among the things that will run on it: the judge's verdict and
             the guardrail checks are both judgments, and an operator reads
             "what decides" before "what it decides about". -->
        <AdminDecidePanel />
        <AdminJudgePanel />
        <AdminCodingAccountsPanel />
        <AdminGuardrailsPanel />
        <AdminOutreachPanel />
      {/if}
      {#if tab === 'git'}
        <!-- The hosts first: requests and PR bases are about repos on them. -->
        <AdminGitHostsPanel bind:adding={addingHost} />
        <AdminRepoCreationSection />
        <AdminRepoFlowSection />
      {/if}
      {#if tab === 'search'}
        <!-- The workspace search URL first: research reads it, and the agent
             setting below can pick it (SearXNG) by reference. -->
        <SearchEndpointPanel />
        <AgentWebSearchPanel />
      {/if}
      {#if tab === 'retrieval'}<RetrievalPanel />{/if}
      {#if tab === 'storage'}<StoragePanel />{/if}
      <!-- TWO DIFFERENT NOUNS ON ONE TAB, in the order an operator needs them: the
           inventory answers "is this instance healthy", the panel below answers
           "what may my agents spend". Sharing a tab is right — they are both
           credentials — and sharing a heading would not be. -->
      {#if tab === 'secrets'}
        <SecretsPanel />
        <WorkspaceSecretsPanel bind:adding={addingCredentials} />
      {/if}
      {#if tab === 'security'}
        <AdminEncryptionPanel />
        <AdminUpdatePanel />
        <AdminSettingsPanel />
      {/if}
      {#if tab === 'people'}
        <AdminPeoplePanel />
        <AdminInvitesPanel bind:inviting />
        {#if perms}
          <AdminMemberDefaultsPanel {perms} />
        {:else if permsQuery.isError}
          <!-- Silence here reads as "no permission model configured". Say so. -->
          <SettingsSection title="Member permissions">
            <SettingsGroup>
              <QueryError
                variant="compact"
                error={permsQuery.error}
                title="Could not load member permissions"
                onRetry={() => void permsQuery.refetch()}
              />
            </SettingsGroup>
          </SettingsSection>
        {/if}
        <AdminPasswordAccountsPanel />
      {/if}
      </div>
      {/key}
  </SettingsPage>
{/if}
