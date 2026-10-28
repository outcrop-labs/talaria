<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import HealthDot from './HealthDot.svelte'
  import { agentProbeNote, useSearchEndpoint } from './search'

  // THE WORKSPACE SEARCH URL: an operator's own SearXNG-compatible endpoint.
  // Research searches it, and the fleet render hands it to every agent as
  // SEARXNG_URL. Two dots, because those are two vantage points: the app's
  // own probe, and the same probe run inside a running agent against the
  // URL as the agents dial it — a SearXNG on the host's loopback answers the
  // first and refuses the second. The PUT re-tests after saving, so the dots
  // answer for the value just written. SEARXNG_URL in the api's environment
  // wins.
  const qc = useQueryClient()
  const query = useSearchEndpoint()
  let busy = $state(false)

  const save = async (url: string) => {
    busy = true
    try {
      await putJson('/api/admin/search', { url })
    } catch (e) {
      toastError('Search URL not saved', e)
    } finally {
      busy = false
    }
    await qc.invalidateQueries({ queryKey: ['admin-search'] })
    await qc.invalidateQueries({ queryKey: ['admin-search-agents'] })
  }
</script>

<Panel>
  <QueryState {query} errorTitle="Could not load the search URL" errorVariant="compact">
    {#snippet skeleton()}
      <Skeleton class="mb-3 h-4 w-28 rounded-full" />
      <Skeleton class="h-8 w-96 rounded-md" />
    {/snippet}
    {#snippet children(s)}
      <SectionHeader
        title="Search URL"
        info="A SearXNG-compatible JSON endpoint you run (its settings.yml must allow format: json). Empty means this workspace has no endpoint of that shape."
      />
      <p class="mb-3 max-w-2xl font-sans text-xs text-muted">
        Research searches here when no search MCP server is registered, and every agent's Hermes receives it as
        SEARXNG_URL. It is tested from both places: the app, and inside a running agent.
      </p>
      <div class="flex flex-wrap items-center gap-3">
        <Input
          size="sm"
          value={s.fromEnv ? s.url : s.stored}
          disabled={s.fromEnv || busy}
          onblur={(e) => e.currentTarget.value.trim() !== s.stored && void save(e.currentTarget.value.trim())}
          placeholder="https://search.example.com"
          class="w-96 font-[var(--font-mono)]"
        />
        {#if s.url}
          <HealthDot ok={s.reachable} label="from app" />
          <HealthDot
            ok={s.agentReachable}
            label="from agents"
            title={s.probedFrom ? `Probed inside ${s.probedFrom}${s.agentUrl ? `, dialling ${s.agentUrl}` : ''}` : undefined}
          />
        {/if}
      </div>
      {#if s.fromEnv}
        <p class="mt-2 font-sans text-xs text-muted">Set by SEARXNG_URL in the api's environment, which wins over this field.</p>
      {:else if s.url && !s.reachable && s.error}
        <p class="mt-2 font-sans text-xs text-danger">From the app: {s.error}</p>
      {/if}
      {#if agentProbeNote(s)}
        {@const note = agentProbeNote(s)!}
        <p class="mt-2 font-sans text-xs {note.tone === 'danger' ? 'text-danger' : 'text-muted'}">{note.text}</p>
      {/if}
    {/snippet}
  </QueryState>
</Panel>
