<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import { needsKeyFirst, renderSummary, useAgentWebSearch, type AgentWebSearch, type WebBackend } from './search'

  // AGENT WEB SEARCH: the one provider every agent's own Hermes searches
  // with. Picks autosave; a vendor key is the one explicit save (a secret
  // being committed). A keyed vendor with no stored key is held as a draft
  // until its key arrives, because the server refuses the pick without one.
  type Field = 'backend' | 'extractBackend'

  const qc = useQueryClient()
  const query = useAgentWebSearch()
  let draft = $state<{ field: Field; name: string } | null>(null)
  let typed = $state<Record<string, string>>({})
  let busy = $state(false)

  const put = async (body: Record<string, unknown>): Promise<boolean> => {
    busy = true
    try {
      qc.setQueryData(['admin-search-agents'], await putJson<AgentWebSearch>('/api/admin/search/agents', body))
      return true
    } catch (e) {
      toastError('Agent web search not saved', e)
      return false
    } finally {
      busy = false
    }
  }

  const pick = (cfg: AgentWebSearch, field: Field, name: string) => {
    if (name && needsKeyFirst(cfg, name)) {
      draft = { field, name }
      return
    }
    draft = null
    void put({ [field]: name || null })
  }

  const saveKey = async (vendor: string) => {
    const body: Record<string, unknown> = { keys: { [vendor]: typed[vendor] ?? '' } }
    if (draft?.name === vendor) body[draft.field] = draft.name
    if (await put(body)) {
      typed = { ...typed, [vendor]: '' }
      if (draft?.name === vendor) draft = null
    }
  }

  const shown = (cfg: AgentWebSearch, field: Field) => (draft?.field === field ? draft.name : (cfg[field] ?? ''))

  /** The picked vendors (drafts included) that take a key, search first. */
  const keyed = (cfg: AgentWebSearch): WebBackend[] => {
    const out: WebBackend[] = []
    for (const name of [shown(cfg, 'backend'), shown(cfg, 'extractBackend')]) {
      const b = cfg.backends.find((x) => x.name === name)
      if (b?.keyEnv && !out.some((o) => o.name === b.name)) out.push(b)
    }
    return out
  }
</script>

<Panel>
  <QueryState {query} errorTitle="Could not load agent web search" errorVariant="compact">
    {#snippet skeleton()}
      <Skeleton class="mb-3 h-4 w-32 rounded-full" />
      <SkeletonRows rows={3} />
    {/snippet}
    {#snippet children(cfg)}
      <SectionHeader
        title="Agent web search"
        info="Rendered into every agent's Hermes config as web.search_backend / web.extract_backend, with the vendor key in the agent's environment. The key is stored encrypted and never shown again."
      />
      <p class="mb-3 max-w-2xl font-sans text-xs text-muted">
        The provider every agent searches the web with, through its own Hermes. Research does not use it: research
        searches with the search URL above or a registered search MCP server. Left on Hermes's default, Talaria renders
        nothing and recommends no vendor. Changes reach each agent at its next roll.
      </p>
      <div class="space-y-3">
        <div class="flex flex-wrap items-center gap-3">
          <label for="aws-search" class="w-28 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Search</label>
          <Select id="aws-search" size="sm" value={shown(cfg, 'backend')} disabled={busy} onchange={(e) => pick(cfg, 'backend', e.currentTarget.value)} class="w-64">
            <option value="">Hermes's default</option>
            {#each cfg.backends as b (b.name)}
              <option value={b.name} disabled={b.name === 'searxng' && !cfg.searchUrl}>{b.label}</option>
            {/each}
          </Select>
        </div>
        <div class="flex flex-wrap items-center gap-3">
          <label for="aws-extract" class="w-28 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Read pages</label>
          <Select id="aws-extract" size="sm" value={shown(cfg, 'extractBackend')} disabled={busy} onchange={(e) => pick(cfg, 'extractBackend', e.currentTarget.value)} class="w-64">
            <option value="">Hermes decides</option>
            {#each cfg.backends.filter((b) => b.extract) as b (b.name)}
              <option value={b.name}>{b.label}</option>
            {/each}
          </Select>
        </div>
        {#each keyed(cfg) as b (b.name)}
          <div transition:slide={{ duration: 150 }} class="flex flex-wrap items-center gap-3">
            <label for="aws-key-{b.name}" class="w-28 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{b.label} key</label>
            <Input
              id="aws-key-{b.name}"
              size="sm"
              type="password"
              autocomplete="off"
              value={typed[b.name] ?? ''}
              oninput={(e) => (typed = { ...typed, [b.name]: e.currentTarget.value })}
              onkeydown={submitOnEnter(() => typed[b.name] && !busy && void saveKey(b.name))}
              placeholder={cfg.keys[b.name] ? 'key saved (type to replace)' : b.keyEnv}
              class="w-64 font-[var(--font-mono)]"
            />
            {#if typed[b.name]}
              <Button size="sm" disabled={busy} onclick={() => void saveKey(b.name)}>Save key</Button>
            {:else if draft?.name === b.name}
              <span class="font-sans text-xs text-muted">{b.label} needs its key before agents can use it.</span>
            {/if}
          </div>
          {#if b.sdkOnFirstUse}
            <p class="ml-[7.75rem] max-w-xl font-sans text-xs text-muted">
              Hermes does not ship the {b.label} SDK. It installs it into the agent on first use, and the agent may need a
              restart before {b.label} answers.
            </p>
          {/if}
        {/each}
        <Checkbox
          checked={cfg.allowKeylessPublicTiers}
          disabled={busy}
          onChange={(checked) => void put({ allowKeylessPublicTiers: checked })}
          label="Allow Hermes's keyless public tiers"
          title="Off: once a provider or search URL is set, Hermes never falls back to the anonymous free tiers of Exa, Parallel, Firecrawl or Keenable, and a failing provider is not retried through them. On: Hermes keeps that fallback."
        />
        <div class="border-t border-line-subtle pt-3 font-mono text-[10px] tracking-[0.05em] text-muted">
          Agents get: {renderSummary(cfg)}
        </div>
        {#each cfg.rendered.warnings as w, i (i)}
          <p class="max-w-2xl font-sans text-xs text-warning">{w.replace(/^agent web search: /, '')}</p>
        {/each}
      </div>
    {/snippet}
  </QueryState>
</Panel>
