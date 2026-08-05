<script lang="ts">
  import { searchParams } from 'sv-router'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { useEndpoints } from '@/lib/models'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsAddProviderModal from './ModelsAddProviderModal.svelte'
  import ModelsEndpointCard from './ModelsEndpointCard.svelte'
  import ModelsEndpointModal from './ModelsEndpointModal.svelte'
  import ModelsMemberAccessPanel from './ModelsMemberAccessPanel.svelte'
  import ModelsPlatformAgentsPanel from './ModelsPlatformAgentsPanel.svelte'
  import ModelsRolesPanel from './ModelsRolesPanel.svelte'
  import { MODEL_TABS, type ModelsTab } from './models'

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const endpointsQuery = useEndpoints(() => isAdmin)
  const endpoints = $derived(endpointsQuery.data ?? [])
  // /models?tab=roles deep-links a tab.
  const tab: ModelsTab = $derived.by(() => {
    const t = MODEL_TABS.find((v) => v.id === searchParams.get('tab'))
    return t && t.id !== 'models' ? t.id : 'models'
  })
  const setTab = (t: ModelsTab) => {
    if (t === 'models') searchParams.delete('tab')
    else searchParams.set('tab', t)
  }
  let adding = $state(false)
  // A just-added provider: jump straight into its manage modal so models get
  // picked from the provider's live catalog (endpoints start with none).
  let manageId = $state<string | null>(null)
  const managing = $derived(manageId ? endpoints.find((e) => e.id === manageId) : undefined)

  const local = $derived(endpoints.filter((e) => e.class === 'local'))
  const cloud = $derived(endpoints.filter((e) => e.class === 'cloud'))
</script>

<!-- §8 section header: 10px mono uppercase ink-dim + right-aligned mono count. -->
{#snippet section(title: string, eps: LlmEndpoint[])}
  {#if eps.length > 0}
    <div>
      <SectionHeader class="mb-2" {title} action={String(eps.length).padStart(2, '0')} />
      <!-- data-cards: the hook the pane's data-stagger-items selector targets —
           endpoint cards cascade on entrance; skeletons/other panes don't. -->
      <div class="space-y-3" data-cards>
        {#each eps as e (e.id)}
          <ModelsEndpointCard ep={e} />
        {/each}
      </div>
    </div>
  {/if}
{/snippet}

{#if session.data && !isAdmin}
  <EmptyState icon="▤" title="Admins only" />
{:else}
  <div class="h-full overflow-y-auto p-8">
    <!-- Page content entrance: header row → tab strip → pane rise in sequence
         (ANIMATIONS.md). The keyed pane keeps its own fly on tab switch and
         stays unstaggered inside — one level only. -->
    <div use:staggerIn class="mx-auto max-w-4xl space-y-6">
      <ViewHeader title="Models">
        {#snippet actions()}
          {#if tab === 'models'}
            <Button size="sm" onclick={() => (adding = true)}>
              Add provider
            </Button>
          {/if}
        {/snippet}
      </ViewHeader>

      <Tabs items={MODEL_TABS} value={tab} onChange={setTab} />

      <!-- Tab-pane grammar: rise in on switch, no exit. On MOUNT the pane is a
           section whose items cascade: endpoint cards (the [data-cards] hook)
           rise 30ms apart while the frame fades. The selector deliberately
           misses skeletons and the other tabs' panels — those must not
           animate row-by-row. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} class="space-y-6" data-stagger-items="[data-cards] > *">
      {#if tab === 'models'}
        {#if endpointsQuery.isPending}
          <div class="space-y-3">
            <SkeletonCard />
            <SkeletonCard delay={0.15} />
          </div>
        {:else if !endpointsQuery.data}
          <!-- The registry read failed. "No model backends yet — Add a
               provider" invites an admin to re-add providers that already
               exist (and re-enter their keys) because the list 500'd. Same
               guard the Member access panel below already uses. -->
          <QueryError
            error={endpointsQuery.error}
            title="Could not load your model backends"
            onRetry={() => void endpointsQuery.refetch()}
          />
        {:else if endpoints.length === 0}
          <EmptyState
            icon="▤"
            title="No model backends yet"
            hint="Add a provider, or import your stack on the Agents page to seed them."
          >
            {#snippet action()}
              <Button size="sm" onclick={() => (adding = true)}>Add provider</Button>
            {/snippet}
          </EmptyState>
        {:else}
          {@render section('Self-hosted: your hardware & on-prem', local)}
          {@render section('Cloud', cloud)}
        {/if}
      {/if}
      {#if tab === 'roles'}<ModelsRolesPanel />{/if}
      {#if tab === 'platform'}<ModelsPlatformAgentsPanel />{/if}
      {#if tab === 'access'}<ModelsMemberAccessPanel />{/if}
      </div>
      {/key}

      {#if adding}<ModelsAddProviderModal open={adding} onClose={() => (adding = false)} onAdded={(id) => (manageId = id)} />{/if}
      {#if managing}<ModelsEndpointModal ep={managing} onClose={() => (manageId = null)} />{/if}
    </div>
  </div>
{/if}
