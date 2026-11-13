<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useSavedCue } from '@/components/admin/saved-cue'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { cn } from '@/lib/cn'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import RowStatus from './RowStatus.svelte'
  import { type SearchEndpoint, agentProbeNote, useSearchEndpoint } from './search'

  // THE WORKSPACE SEARCH URL: an operator's own SearXNG-compatible endpoint.
  // Research searches it, and the fleet render hands it to every agent as
  // SEARXNG_URL. Two status rows, because those are two vantage points: the
  // app's own probe, and the same probe run inside a running agent against the
  // URL as the agents dial it — a SearXNG on the host's loopback answers the
  // first and refuses the second. The PUT re-tests after saving and answers
  // with the GET's body, so its answer IS the panel's new state: refetching
  // would run both probes (an agent exec among them) a second time.
  // SEARXNG_URL in the api's environment wins.
  const qc = useQueryClient()
  const autosave = useSavedCue()
  const query = useSearchEndpoint()
  let busy = $state(false)

  const save = async (url: string) => {
    busy = true
    try {
      qc.setQueryData<SearchEndpoint>(['admin-search'], await putJson<SearchEndpoint>('/api/admin/search', { url }))
      autosave.flash()
    } catch (e) {
      toastError('Search URL not saved', e)
      // The field still holds what was typed; read back what is stored.
      await qc.invalidateQueries({ queryKey: ['admin-search'] })
    } finally {
      busy = false
    }
    // The render summary below reads the URL too.
    await qc.invalidateQueries({ queryKey: ['admin-search-agents'] })
  }
</script>

<SettingsSection
  title="Search endpoint"
  description="Your own SearXNG-compatible search service, used by Research and by every agent."
  info="A SearXNG-compatible JSON endpoint you run (its settings.yml must allow format: json). Research searches here when no search MCP server is registered, and every agent receives it as SEARXNG_URL. Empty means this workspace has no endpoint of that shape."
>
  <SettingsGroup>
    <QueryState {query} errorTitle="Could not load the search URL" errorVariant="compact">
      {#snippet skeleton()}
        <div class="px-5 py-4"><SkeletonRows rows={2} /></div>
      {/snippet}
      {#snippet children(s)}
        {@const note = agentProbeNote(s)}
        <SettingsRow label="URL" as="label">
          {#snippet hint()}
            {#if s.fromEnv}
              Set by <span class="font-mono text-xs">SEARXNG_URL</span> in the API's environment, which wins over this field.
            {:else}
              Saved when you leave the field, then tested from both places.
            {/if}
          {/snippet}
          <!-- Locked by SEARXNG_URL reads locked: Button's dim reading (opacity
               50, which keeps the URL legible in both themes) and a not-allowed
               cursor. It keys on fromEnv, not on `disabled`: the field is also
               disabled for the length of a save, and dimming it then would only
               flicker. It rides here because the Input primitive has no
               disabled state of its own, and giving it one would also dim every
               board viewer's read-only ticket title, labels and statuses. -->
          <Input
            size="sm"
            value={s.fromEnv ? s.url : s.stored}
            disabled={s.fromEnv || busy}
            onblur={(e) => e.currentTarget.value.trim() !== s.stored && void save(e.currentTarget.value.trim())}
            placeholder="https://search.example.com"
            class={cn('w-[280px] font-mono', s.fromEnv && 'cursor-not-allowed opacity-50')}
          />
        </SettingsRow>
        {#if s.url}
          <SettingsRow
            label="From the app"
            hint={!s.reachable && s.error ? s.error : 'The address Research searches.'}
          >
            {#snippet status()}
              <RowStatus status={s.reachable ? 'ok' : 'danger'}>{s.reachable ? 'Reachable' : 'Unreachable'}</RowStatus>
            {/snippet}
          </SettingsRow>
          <SettingsRow label="From agents">
            {#snippet hint()}
              {#if note}
                {note.text}
              {:else if s.probedFrom}
                Checked inside {s.probedFrom}{#if s.agentUrl}, at <span class="font-mono text-xs">{s.agentUrl}</span>{/if}.
              {:else}
                The same check, run inside a running agent.
              {/if}
            {/snippet}
            {#snippet status()}
              <RowStatus status={s.agentReachable === null ? 'idle' : s.agentReachable ? 'ok' : 'danger'}>
                {s.agentReachable === null ? 'Not checked' : s.agentReachable ? 'Reachable' : 'Unreachable'}
              </RowStatus>
            {/snippet}
          </SettingsRow>
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
