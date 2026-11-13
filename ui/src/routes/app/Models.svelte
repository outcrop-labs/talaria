<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { navigate, route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { fly } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { useEndpoints } from '@/lib/models'
  import ModelsAddProviderModal from './ModelsAddProviderModal.svelte'
  import ModelsEndpointCard from './ModelsEndpointCard.svelte'
  import ModelsEndpointModal from './ModelsEndpointModal.svelte'
  import ModelsFitnessPanel from './ModelsFitnessPanel.svelte'
  import ModelsMemberAccessPanel from './ModelsMemberAccessPanel.svelte'
  import ModelsRolesPanel from './ModelsRolesPanel.svelte'
  import { MODEL_TABS, type ModelsTab } from './models'

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const endpointsQuery = useEndpoints(() => isAdmin)
  const endpoints = $derived(endpointsQuery.data ?? [])
  // /models/roles deep-links a tab.
  // THE URL IS THE TAB — /models and /models/<tab>. lib/route-tabs.ts holds
  // the shared fallback rule.
  const tab = $derived(tabFromPath(route.pathname, '/models', MODEL_TABS.map((t) => t.id), 'models'))
  const setTab = (t: ModelsTab) => {
    if (t === 'models') void navigate('/models')
    else void navigate('/models/:tab', { params: { tab: t } })
  }
  let adding = $state(false)
  // A just-added provider: jump straight into its manage modal so models get
  // picked from the provider's live catalog (endpoints start with none).
  let manageId = $state<string | null>(null)
  const managing = $derived(manageId ? endpoints.find((e) => e.id === manageId) : undefined)

  const local = $derived(endpoints.filter((e) => e.class === 'local'))
  const cloud = $derived(endpoints.filter((e) => e.class === 'cloud'))
</script>

{#if session.data && !isAdmin}
  <EmptyState icon="▤" title="Admins only" />
{:else}
  <SettingsPage
    title="Models"
    blurb="The model providers your agents can use."
    tabs={MODEL_TABS}
    {tab}
    onTab={setTab}
    staggerItems="[data-cards] > *"
  >
        {#snippet actions()}
          {#if tab === 'models'}
            <Button size="sm" onclick={() => (adding = true)}>
              Add provider
            </Button>
          {/if}
        {/snippet}

      <!-- Tab-pane grammar: rise in on switch, no exit. On MOUNT the pane is a
           section whose items cascade: endpoint cards (the [data-cards] hook)
           rise 30ms apart while the frame fades. The selector deliberately
           misses skeletons and the other tabs' panels — those must not
           animate row-by-row. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} class="space-y-6" data-stagger-items="[data-cards] > *">
      {#if tab === 'models'}
        {#if endpointsQuery.isPending}
          <SettingsGroup>
            <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
          </SettingsGroup>
        {:else if !endpointsQuery.data}
          <!-- The registry read failed. "No model backends yet — Add a
               provider" invites an admin to re-add providers that already
               exist (and re-enter their keys) because the list 500'd. Same
               guard the Member access panel below already uses. -->
          <QueryError
            error={endpointsQuery.error}
            title="Could not load your providers"
            onRetry={() => void endpointsQuery.refetch()}
          />
        {:else if endpoints.length === 0}
          <EmptyState
            icon="▤"
            title="No providers yet"
            hint="Add a provider, or import your stack on the Agents page."
          >
            {#snippet action()}
              <Button size="sm" variant="outline" onclick={() => (adding = true)}>Add provider</Button>
            {/snippet}
          </EmptyState>
        {:else}
          <!-- data-cards: the hook the page's stagger selector targets, so
               provider rows cascade on entrance and skeletons do not. -->
          <SettingsGroup data-cards>
            {#each [...local, ...cloud] as e (e.id)}
              <ModelsEndpointCard ep={e} />
            {/each}
          </SettingsGroup>
        {/if}
      {/if}
      {#if tab === 'roles'}<ModelsRolesPanel />{/if}
      {#if tab === 'fitness'}<ModelsFitnessPanel />{/if}
      {#if tab === 'access'}<ModelsMemberAccessPanel />{/if}
      </div>
      {/key}

      {#if adding}<ModelsAddProviderModal open={adding} onClose={() => (adding = false)} onAdded={(id) => (manageId = id)} />{/if}
      {#if managing}<ModelsEndpointModal ep={managing} onClose={() => (manageId = null)} />{/if}
  </SettingsPage>
{/if}
