<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** The HOSTING domain — where this deployment lives. Verified by a self-
   *  fetch round trip (DNS + routing + TLS must land on this instance), and
   *  then used as the canonical base URL (stable OAuth callbacks, links). */
  const qc = useQueryClient()
  // `null` is a real answer here — no hosting domain configured yet, and the
  // form below invites you to set one. A FAILED read used to produce the same
  // null, so a blip showed a configured instance its "set a domain" form and a
  // Save would have rewritten live OAuth callbacks. Now only the server's own
  // `instance: null` gets there; a non-2xx becomes an error.
  const query = createQuery(() => ({
    queryKey: ['instance-domain'],
    queryFn: async (): Promise<{ domain: string; verified: boolean; verifiedAt: string | null } | null> =>
      (await getJson<{ instance: { domain: string; verified: boolean; verifiedAt: string | null } | null }>('/api/admin/instance')).instance,
  }))
  const data = $derived(query.data)
  let draft = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['instance-domain'] })

  const post = async (body: Record<string, unknown>) => {
    busy = true
    error = null
    try {
      // PUT writes the domain config; POST runs the verify action.
      const send = 'verify' in body ? postJson : putJson
      await send<{ ok: true }>('/api/admin/instance', body)
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
</script>

<Panel class="mt-4">
  <SectionHeader
    title="Instance domain"
    info="Where THIS Talaria deployment is hosted (e.g. talaria.yourcompany.com), separate from your email sign-up domains below. Verification round-trips through the domain and confirms it reaches this exact instance. Once verified it becomes the canonical base URL: OAuth apps get one stable callback, links use it."
  />
  {#if query.isPending}
    <SkeletonRows rows={1} />
  {:else if data === undefined}
    <QueryError
      variant="inline"
      error={query.error}
      title="Could not load the instance domain"
      onRetry={() => void query.refetch()}
    />
  {:else if data}
    <div class="flex items-center gap-2">
      <span class="font-mono text-[13px] text-fg">{data.domain}</span>
      {#if data.verified}
        <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-success">
          <StatusDot status="ok" />
          verified · routes to this instance
        </span>
      {:else}
        <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-warning">
          <StatusDot status="warn" />
          unverified
        </span>
      {/if}
      <span class="flex-1"></span>
      {#if !data.verified}
        <Button size="sm" variant="outline" disabled={busy} onclick={() => void post({ verify: true })}>
          {busy ? 'Checking' : 'Verify'}
        </Button>
      {/if}
      <button
        type="button"
        title="Clear the hosting domain"
        onclick={() => void post({ domain: null })}
        class="text-muted transition-colors hover:text-danger"
      >
        <Trash2 size={13} />
      </button>
    </div>
  {:else}
    <div class="flex items-center gap-2">
      <Input
        size="sm"
        bind:value={draft}
        onkeydown={(e) => e.key === 'Enter' && draft.trim() && void post({ domain: draft.trim() })}
        placeholder="talaria.yourcompany.com (where this instance is hosted)"
        class="w-96"
      />
      <Button size="sm" disabled={!draft.trim() || busy} onclick={() => void post({ domain: draft.trim() })}>
        Set domain
      </Button>
    </div>
  {/if}
  {#if error}
    <div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">
      {error}
    </div>
  {/if}
</Panel>
