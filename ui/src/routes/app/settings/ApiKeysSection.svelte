<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getJson, postJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { fade, slide } from '@/lib/motion'
  import { pushToast } from '@/lib/toast.svelte'

  interface ApiKey {
    id: string
    name: string
    prefix: string
    createdAt: string
    lastUsedAt: string | null
    revokedAt: string | null
  }

  // Personal keys for the Talaria LLM gateway — one org endpoint over the whole
  // model stack. The secret shows exactly once at mint time.

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['api-keys'],
    queryFn: (): Promise<{ keys: ApiKey[]; canMint: boolean }> => getJson<{ keys: ApiKey[]; canMint: boolean }>('/api/keys'),
  }))
  const data = $derived(query.data)
  const isLoading = $derived(query.isLoading)
  let name = $state('')
  let minted = $state<string | null>(null)
  let err = $state<string | null>(null)
  const keys = $derived((data?.keys ?? []).filter((k) => !k.revokedAt))
  const baseUrl = typeof window !== 'undefined' ? `${window.location.origin}/api/llm/v1` : '/api/llm/v1'

  const mint = async () => {
    err = null
    try {
      const j = await postJson<{ key: ApiKey; secret: string }>('/api/keys', { name: name.trim() || 'default' })
      minted = j.secret
      name = ''
      await qc.invalidateQueries({ queryKey: ['api-keys'] })
    } catch (e) {
      err = errorMessage(e)
    }
  }

  const revoke = async (id: string) => {
    if (!(await confirm({ title: 'Revoke key', message: 'Revoke this key? Anything using it stops working immediately.', confirmLabel: 'Revoke', danger: true }))) return
    try {
      await delJson(`/api/keys/${id}`)
    } catch (e) {
      // Row button; the err line belongs to the mint form below.
      pushToast({ title: 'Revoke failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }
</script>

<Panel as="section">
  <SectionHeader
    class="mb-4"
    title="API keys"
    info={`Connect external tools to the org's model stack: base URL ${baseUrl}, any model from /models (or endpoint/model to pin a backend).`}
  />

  {#if isLoading}
    <!-- Hold BOTH the key list and the canMint branch — flashing "API keys
         are not enabled for your account" mid-load is actively wrong. -->
    <div aria-hidden="true">
      <SkeletonRows rows={2} class="mb-4" />
      <div class="flex items-center gap-2">
        <Skeleton class="h-9 flex-1" />
        <Skeleton class="h-9 w-24" />
      </div>
    </div>
  {:else if !data}
    <!-- The skeleton comment above holds after load too: "API keys are not
         enabled for your account" is a permission verdict, and a failed
         /api/keys never delivered one. -->
    <QueryError variant="compact" error={query.error} title="Could not load your API keys" onRetry={() => void query.refetch()} />
  {:else}
    {#if keys.length > 0}
      <div class="mb-4 divide-y divide-line-subtle">
        {#each keys as k (k.id)}
          <div
            in:fade={{ duration: 150 }}
            out:slide={{ duration: 150 }}
            class="-mx-2 flex items-center gap-3 rounded-md px-2 py-3 text-sm transition-colors dither-fill"
          >
            <span class="w-28 shrink-0 truncate font-medium text-fg">{k.name}</span>
            <code class="shrink-0 font-mono text-[11px] text-muted">{k.prefix}</code>
            <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">
              {k.lastUsedAt ? `used ${relativeTime(k.lastUsedAt)}` : 'never used'}
            </span>
            <Button variant="danger-outline" size="sm" class="shrink-0" onclick={() => void revoke(k.id)}>
              Revoke
            </Button>
          </div>
        {/each}
      </div>
    {/if}

    {#if data?.canMint}
      <!-- A lone field + button stretched to the full page width reads as
           broken. The page FRAME is what stays consistent between views; a
           field inside it is sized by what it holds. -->
      <div class="flex max-w-md items-center gap-2">
        <Input
          size="sm"
          bind:value={name}
          placeholder="key name (e.g. opencode)"
          onkeydown={(e) => e.key === 'Enter' && void mint()}
        />
        <Button size="sm" onclick={() => void mint()}>
          Create key
        </Button>
      </div>
    {:else}
      <div class="text-xs text-muted">API keys are not enabled for your account. Ask an admin.</div>
    {/if}
  {/if}
  {#if minted}
    <div transition:slide={{ duration: 150 }} class="mt-3 rounded-md border border-accent/40 bg-raised p-3">
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-warning">Copy it now: it won't be shown again</div>
      <code class="break-all font-mono text-xs text-fg">{minted}</code>
    </div>
  {/if}
  {#if err}
    <div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">
      {err}
    </div>
  {/if}
</Panel>
