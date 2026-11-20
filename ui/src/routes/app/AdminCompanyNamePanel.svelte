<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import { errorMessage, getJson, putJson } from '@/lib/fetch-json'

  /** The instance's DISPLAY name — the second half of every browser tab
   *  ("Talaria - <name>"). Purely cosmetic; nothing else derives from it. A row
   *  of the Instance card, above the address this instance is hosted at. */
  const qc = useQueryClient()
  // `null` is a real answer (no name configured); undefined means the read
  // failed, and a failed read must not render as "no name" the way a save
  // could then quietly overwrite nothing.
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
  const autosave = useAutosave()
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
      autosave.flash()
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }

  // An emptied field clears the name; anything else stores what was typed.
  const commit = () => {
    if (data === undefined || draft === (data ?? '')) return
    if (draft.trim()) void save({ companyName: draft })
    else if (data) void save({ companyName: null })
  }
</script>

<SettingsRow
  label="Browser tab name"
  info="People's browser tabs read 'Talaria' until you set a name, then 'Talaria - your name'. Nothing else uses it. Empty the field to clear it."
  as="label"
  saved={autosave.saved()}
>
  {#snippet hint()}
    {#if error}<span class="text-danger">{error}</span>{:else}Shown in everyone's browser tab, after Talaria.{/if}
  {/snippet}
  {#if query.isPending}
    <Skeleton class="h-9 w-[280px]" />
  {:else if data === undefined}
    <QueryError
      variant="inline"
      error={query.error}
      title="Could not load the browser tab name"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <Input
      size="sm"
      class="w-[280px]"
      bind:value={draft}
      disabled={busy}
      onblur={commit}
      onkeydown={inlineEditKeys(() => (draft = data ?? ''))}
      placeholder="Acme Corp"
    />
  {/if}
</SettingsRow>
