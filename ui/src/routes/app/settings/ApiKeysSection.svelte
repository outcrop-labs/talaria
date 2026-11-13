<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { KeyRound } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJsonGone, errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { fade, slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'

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

  // Personal keys for the Talaria LLM gateway — one org endpoint over the whole
  // model stack. The secret shows exactly once at mint time. Each key carries
  // its OWN ceilings (#265): a spend cap over the org budget window and a
  // per-minute request limit — self-imposed, so a leaked or runaway key burns
  // only what its owner decided it may.

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['api-keys'],
    queryFn: (): Promise<{ keys: ApiKey[]; canMint: boolean }> => getJson<{ keys: ApiKey[]; canMint: boolean }>('/api/keys'),
  }))
  const data = $derived(query.data)
  let name = $state('')
  let minted = $state<string | null>(null)
  let err = $state<string | null>(null)
  const keys = $derived((data?.keys ?? []).filter((k) => !k.revokedAt))
  const baseUrl = typeof window !== 'undefined' ? `${window.location.origin}/api/llm/v1` : '/api/llm/v1'

  // One limits editor open at a time; the draft is seeded from the row, blank
  // meaning unlimited — the same spelling the API uses (null).
  let limitsFor = $state<string | null>(null)
  let draft = $state({ tokens: '', usd: '', rpm: '' })

  const limitsSummary = (k: ApiKey) => {
    const parts: string[] = []
    if (k.spendCapTokens) parts.push(`${k.spendCapTokens.toLocaleString()} tok`)
    if (k.spendCapUsd) parts.push(`$${k.spendCapUsd}`)
    if (k.rateLimitPerMinute) parts.push(`${k.rateLimitPerMinute}/min`)
    return parts.join(' · ') || 'no limits'
  }

  const openLimits = (k: ApiKey) => {
    limitsFor = limitsFor === k.id ? null : k.id
    draft = {
      tokens: k.spendCapTokens ? String(k.spendCapTokens) : '',
      usd: k.spendCapUsd ? String(k.spendCapUsd) : '',
      rpm: k.rateLimitPerMinute ? String(k.rateLimitPerMinute) : '',
    }
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
      // A 404 is a key that is not this person's any more: nothing to revoke.
      await delJsonGone(`/api/keys/${id}`)
    } catch (e) {
      // Row button; the err line belongs to the mint form below.
      toastError('Revoke failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }
</script>

{#if minted}
  <div transition:slide={{ duration: 150 }}>
    <SettingsGroup>
      <SettingsRow label="Copy your new key now" hint="It will not be shown again." stacked>
        <code class="min-w-0 flex-1 break-all rounded-md border border-line bg-surface px-3 py-2 font-mono text-xs text-fg">{minted}</code>
        <CopyButton value={minted} label="Copy" />
      </SettingsRow>
    </SettingsGroup>
  </div>
{/if}

<SettingsSection title="Your keys">
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load your API keys" errorVariant="compact">
      {#snippet skeleton()}
        <!-- Hold BOTH the key list and the canMint branch: flashing "API keys
             are not enabled for your account" mid-load is actively wrong. -->
        <div aria-hidden="true" class="px-5 py-4">
          <SkeletonRows rows={2} class="mb-4" />
          <div class="flex items-center justify-end gap-3">
            <Skeleton class="h-9 w-[280px]" />
            <Skeleton class="h-9 w-24" />
          </div>
        </div>
      {/snippet}
      {#snippet children(_data)}
        <!-- "API keys are not enabled for your account" is a permission
             verdict, and a failed /api/keys never delivered one. QueryState's
             error branch owns it. -->
        {#each keys as k (k.id)}
          <div in:fade={{ duration: 150 }} out:slide={{ duration: 150 }}>
            <SettingsRow label={k.name}>
              {#snippet icon()}<KeyRound size={16} />{/snippet}
              {#snippet meta()}<code class="shrink-0 font-mono text-xs text-muted">{k.prefix}</code>{/snippet}
              {#snippet hint()}
                {k.lastUsedAt ? `Used ${relativeTime(k.lastUsedAt)}` : 'Never used'} · {limitsSummary(k)}
              {/snippet}
              <Button variant="outline" size="sm" onclick={() => openLimits(k)}>
                {limitsFor === k.id ? 'Close' : 'Limits'}
              </Button>
              <SettingsRowMenu
                label={`Actions for ${k.name}`}
                items={[{ label: 'Revoke key', danger: true, onSelect: () => void revoke(k.id) }]}
              />
            </SettingsRow>
            {#if limitsFor === k.id}
              <div transition:slide={{ duration: 150 }}>
                <div class="border-t border-line-subtle px-5 py-4">
                  <div class="flex flex-wrap items-end gap-3">
                    <label class="w-40 font-sans text-xs leading-4 text-muted">
                      Token cap
                      <Input size="sm" bind:value={draft.tokens} placeholder="No cap" inputmode="numeric" class="mt-1.5" />
                    </label>
                    <label class="w-40 font-sans text-xs leading-4 text-muted">
                      Dollar cap
                      <Input size="sm" bind:value={draft.usd} placeholder="No cap" inputmode="decimal" class="mt-1.5" />
                    </label>
                    <label class="w-40 font-sans text-xs leading-4 text-muted">
                      Requests per minute
                      <Input size="sm" bind:value={draft.rpm} placeholder="No cap" inputmode="numeric" class="mt-1.5" />
                    </label>
                    <Button size="md" variant="outline" onclick={() => void saveLimits(k)}>Save limits</Button>
                  </div>
                  <p class="mt-2 font-sans text-[13px] leading-5 text-muted">
                    Caps can only tighten the ceiling an admin set. Blank means no cap.
                  </p>
                </div>
              </div>
            {/if}
          </div>
        {/each}

        {#if data?.canMint}
          <SettingsRow label="New key" hint="Name it after the tool that will use it." as="label">
            <Input
              size="sm"
              class="w-[280px]"
              bind:value={name}
              placeholder="opencode"
              onkeydown={(e) => e.key === 'Enter' && void mint()}
            />
            <Button size="md" variant="outline" onclick={() => void mint()}>Create key</Button>
          </SettingsRow>
        {:else}
          <SettingsRow label="Keys are off for your account" hint="Ask an admin to turn them on." />
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
  {#if err}
    <div transition:slide={{ duration: 150 }} class="mt-2 font-sans text-[13px] leading-5 text-danger">{err}</div>
  {/if}
</SettingsSection>

<SettingsSection title="Use a key">
  <SettingsGroup>
    <SettingsRow label="Base URL" hint="Point any OpenAI-compatible tool here. Use a model id from Models.">
      <code class="font-mono text-xs text-muted">{baseUrl}</code>
      <CopyButton value={baseUrl} />
    </SettingsRow>
  </SettingsGroup>
</SettingsSection>
