<script module lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { getJson } from '@/lib/fetch-json'

  interface ApiKey {
    id: string
    name: string
    prefix: string
    createdAt: string
    lastUsedAt: string | null
    revokedAt: string | null
    spendCapTokens: number | null
    spendCapUsd: number | null
    rateLimitPerMinute: number | null
  }
  interface ApiKeys {
    keys: ApiKey[]
    canMint: boolean
  }

  /** The person's keys, and whether they may create one. The page header reads
   *  it too, for its Create key button: one cache entry, one request. */
  export const useApiKeys = (enabled: () => boolean = () => true) =>
    createQuery(() => ({
      queryKey: ['api-keys'],
      queryFn: (): Promise<ApiKeys> => getJson<ApiKeys>('/api/keys'),
      enabled: enabled(),
    }))
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJsonGone, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { fade, slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'

  // Personal keys for the Talaria LLM gateway — one org endpoint over the whole
  // model stack. The secret shows exactly once at mint time. Each key carries
  // its OWN ceilings (#265): a spend cap over the org budget window and a
  // per-minute request limit — self-imposed, so a leaked or runaway key burns
  // only what its owner decided it may.

  /** The create dialog is open. The page header's Create key button sets it. */
  let { creating = $bindable(false) }: { creating?: boolean } = $props()

  const qc = useQueryClient()
  const query = useApiKeys()
  const data = $derived(query.data)
  let name = $state('')
  let minting = $state(false)
  let minted = $state<string | null>(null)
  let err = $state<string | null>(null)
  const keys = $derived((data?.keys ?? []).filter((k) => !k.revokedAt))
  const baseUrl = typeof window !== 'undefined' ? `${window.location.origin}/api/llm/v1` : '/api/llm/v1'

  // One limits editor open at a time; the draft is seeded from the row, blank
  // meaning unlimited — the same spelling the API uses (null).
  type Limits = { tokens: string; usd: string; rpm: string }
  let limitsFor = $state<string | null>(null)
  let draft = $state<Limits>({ tokens: '', usd: '', rpm: '' })
  // A key revoked from its own limits dialog leaves the list, and the dialog with it.
  const limitsKey = $derived(keys.find((k) => k.id === limitsFor) ?? null)

  const limitsOf = (k: ApiKey): Limits => ({
    tokens: k.spendCapTokens ? String(k.spendCapTokens) : '',
    usd: k.spendCapUsd ? String(k.spendCapUsd) : '',
    rpm: k.rateLimitPerMinute ? String(k.rateLimitPerMinute) : '',
  })
  const limitsDirty = $derived.by(() => {
    if (!limitsKey) return false
    const saved = limitsOf(limitsKey)
    return draft.tokens.trim() !== saved.tokens || draft.usd.trim() !== saved.usd || draft.rpm.trim() !== saved.rpm
  })

  const limitsSummary = (k: ApiKey) => {
    const parts: string[] = []
    if (k.spendCapTokens) parts.push(`${k.spendCapTokens.toLocaleString()} token cap`)
    if (k.spendCapUsd) parts.push(`$${k.spendCapUsd.toLocaleString()} cap`)
    if (k.rateLimitPerMinute) parts.push(`${k.rateLimitPerMinute.toLocaleString()} requests a minute`)
    return parts.join(' · ') || 'No limits'
  }

  const openLimits = (k: ApiKey) => {
    limitsFor = k.id
    draft = limitsOf(k)
  }

  const saveLimits = async (k: ApiKey) => {
    // Blank stays null — the wire never has to guess what "no cap" is.
    const num = (s: string) => {
      const v = Number(s.trim())
      return s.trim() !== '' && Number.isFinite(v) && v > 0 ? v : null
    }
    try {
      await putJson(`/api/keys/${k.id}`, {
        spendCapTokens: num(draft.tokens),
        spendCapUsd: num(draft.usd),
        rateLimitPerMinute: num(draft.rpm),
      })
    } catch (e) {
      toastError('Could not save limits', e)
      return
    }
    limitsFor = null
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }

  const mint = async () => {
    if (minting) return
    err = null
    minting = true
    try {
      const j = await postJson<{ key: ApiKey; secret: string }>('/api/keys', { name: name.trim() || 'default' })
      minted = j.secret
      name = ''
      await qc.invalidateQueries({ queryKey: ['api-keys'] })
    } catch (e) {
      err = errorMessage(e)
    } finally {
      minting = false
    }
  }

  // The secret lives only as long as the dialog that shows it.
  const closeCreate = () => {
    creating = false
    minted = null
    name = ''
    err = null
  }

  const revoke = async (id: string) => {
    if (!(await confirm({ title: 'Revoke key', message: 'Revoke this key? Anything using it stops working immediately.', confirmLabel: 'Revoke', danger: true }))) return
    try {
      // A 404 is a key that is not this person's any more: nothing to revoke.
      await delJsonGone(`/api/keys/${id}`)
    } catch (e) {
      toastError('Revoke failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }
</script>

<SettingsSection title="Your keys">
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load your API keys" errorVariant="compact">
      {#snippet skeleton()}
        <!-- Hold the whole card: flashing "Keys are off for your account"
             mid-load is actively wrong. -->
        <div aria-hidden="true" class="px-5 py-4"><SkeletonRows rows={3} /></div>
      {/snippet}
      {#snippet children(_data)}
        <!-- "Keys are off for your account" is a permission verdict, and a
             failed /api/keys never delivered one. QueryState's error branch
             owns it. -->
        {#each keys as k (k.id)}
          <div in:fade={{ duration: 150 }} out:slide={{ duration: 150 }}>
            <SettingsRow label={k.name} onOpen={() => openLimits(k)} openLabel={`Limits for ${k.name}`}>
              {#snippet meta()}<code class="shrink-0 font-mono text-xs text-muted">{k.prefix}</code>{/snippet}
              {#snippet hint()}
                {k.lastUsedAt ? `Used ${relativeTime(k.lastUsedAt)}` : 'Never used'} · {limitsSummary(k)}
              {/snippet}
              <SettingsRowMenu
                label={`Actions for ${k.name}`}
                items={[
                  { label: 'Limits', onSelect: () => openLimits(k) },
                  'sep',
                  { label: 'Revoke key', danger: true, onSelect: () => void revoke(k.id) },
                ]}
              />
            </SettingsRow>
          </div>
        {/each}

        {#if !data?.canMint}
          <SettingsRow label="Keys are off for your account" hint="Ask an admin to turn them on." />
        {:else if keys.length === 0}
          <EmptyState variant="compact" title="No keys yet" hint="Create one for each script or tool that calls Talaria as you." />
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>

<!-- A card of one row, on purpose: loose under the list, the address read as a caption of the last key. -->
<SettingsSection title="Use a key">
  <SettingsGroup>
    <SettingsRow label="Base URL" hint="Point any OpenAI-compatible tool here, with one of your keys and a model id from Models.">
      <code class="font-mono text-xs text-fg">{baseUrl}</code>
      <CopyButton value={baseUrl} title="Copy the base URL" />
    </SettingsRow>
  </SettingsGroup>
</SettingsSection>

{#if creating}
  <!-- The secret exists only in this dialog, so the step that shows it leaves by Done alone. -->
  <Modal open onClose={closeCreate} dismissible={!minted} title={minted ? 'Copy your new key' : 'Create key'} width="max-w-md">
    {#if minted}
      <div class="space-y-3">
        <p class="font-sans text-[13px] leading-5 text-muted">This is the only time the key is shown, so copy it before you close.</p>
        <div class="flex items-center gap-2">
          <code class="min-w-0 flex-1 break-all rounded-md border border-line bg-surface px-3 py-2 font-mono text-xs leading-5 text-fg">{minted}</code>
          <CopyButton value={minted} label="Copy" title="Copy the key" class="shrink-0" />
        </div>
      </div>
    {:else}
      <div class="space-y-5">
        <SettingsField label="Name" for="api-key-name" hint="Name it after the tool that will use it.">
          <Input
            id="api-key-name"
            autofocus
            size="sm"
            bind:value={name}
            placeholder="opencode"
            onkeydown={(e) => e.key === 'Enter' && void mint()}
          />
        </SettingsField>
        {#if err}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{err}</div>{/if}
      </div>
    {/if}
    {#snippet footer()}
      <div class="flex justify-end gap-2">
        {#if minted}
          <Button size="sm" onclick={closeCreate}>Done</Button>
        {:else}
          <Button size="sm" variant="ghost" onclick={closeCreate}>Cancel</Button>
          <Button size="sm" disabled={minting} onclick={() => void mint()}>{minting ? 'Creating' : 'Create key'}</Button>
        {/if}
      </div>
    {/snippet}
  </Modal>
{/if}

{#if limitsKey}
  {@const k = limitsKey}
  <Modal open onClose={() => (limitsFor = null)} title={`Limits for ${k.name}`} width="max-w-md">
    <div class="space-y-5">
      <p class="font-sans text-[13px] leading-5 text-muted">A limit can only tighten what an admin allows. Leave one blank for no limit.</p>
      <SettingsField label="Token cap" for="api-key-tokens">
        <Input id="api-key-tokens" size="sm" bind:value={draft.tokens} placeholder="No cap" inputmode="numeric" />
      </SettingsField>
      <SettingsField label="Dollar cap" for="api-key-usd">
        <Input id="api-key-usd" size="sm" bind:value={draft.usd} placeholder="No cap" inputmode="decimal" />
      </SettingsField>
      <SettingsField label="Requests per minute" for="api-key-rpm">
        <Input id="api-key-rpm" size="sm" bind:value={draft.rpm} placeholder="No limit" inputmode="numeric" />
      </SettingsField>
    </div>
    {#snippet footer()}
      <div class="flex items-center gap-2">
        <DangerLink onClick={() => void revoke(k.id)}>Revoke key</DangerLink>
        <span class="flex-1"></span>
        <Button size="sm" variant="ghost" onclick={() => (limitsFor = null)}>Cancel</Button>
        <Button size="sm" disabled={!limitsDirty} onclick={() => void saveLimits(k)}>Save limits</Button>
      </div>
    {/snippet}
  </Modal>
{/if}
