<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** The instance's DISPLAY name — the second half of every browser tab
   *  ("Talaria - <name>"). Purely cosmetic; nothing else derives from it. */
  const qc = useQueryClient()
  // `null` is a real answer (no name configured); undefined means the read
  // failed, and a failed read must not render as "no name" the way a Save
  // could then quietly overwrite nothing — the QueryError arm holds it.
  const query = createQuery(() => ({
    queryKey: ['company-name'],
    queryFn: async (): Promise<string | null> =>
      (await getJson<{ companyName: string | null }>('/api/admin/instance')).companyName,
  }))
  const data = $derived(query.data)
  let draft = $state('')
  let seeded = $state(false)
  let error = $state<string | null>(null)
  let busy = $state(false)
  // Seed the input once the current name arrives, and only once — later
  // refetches (another admin's change) must not clobber what's being typed.
  $effect(() => {
    if (data !== undefined && !seeded) {
      draft = data ?? ''
      seeded = true
    }
  })
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['company-name'] })
    // The tab titles across everyone's open tabs read the beacon — the
    // save must reach them at the next stale boundary, not never.
    void qc.invalidateQueries({ queryKey: ['instance-branding'] })
  }

  const save = async (body: Record<string, unknown>) => {
    busy = true
    error = null
    try {
      await putJson<{ ok: true }>('/api/admin/instance', body)
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
</script>

<Panel class="mt-4">
  <SectionHeader
    title="Company name"
    info="Names this workspace in people's browser tabs: the tab reads 'Talaria' until you set a name, then 'Talaria - <your name>'. Cosmetic — nothing else derives from it."
  />
  {#if query.isPending}
    <SkeletonRows rows={1} />
  {:else if data === undefined}
    <QueryError
      variant="inline"
      error={query.error}
      title="Could not load the company name"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex items-center gap-2">
      <Input
        size="sm"
        bind:value={draft}
        onkeydown={(e) => e.key === 'Enter' && draft.trim() && void save({ companyName: draft })}
        placeholder="Acme Corp (shows as 'Talaria - Acme Corp')"
        class="w-96"
      />
      <Button size="sm" disabled={!draft.trim() || busy} onclick={() => void save({ companyName: draft })}>
        Save
      </Button>
      {#if data}
        <button
          type="button"
          title="Clear the company name"
          onclick={() => void save({ companyName: null })}
          class="text-muted transition-colors hover:text-danger"
        >
          <Trash2 size={13} />
        </button>
      {/if}
    </div>
  {/if}
  {#if error}
    <div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">
      {error}
    </div>
  {/if}
</Panel>
