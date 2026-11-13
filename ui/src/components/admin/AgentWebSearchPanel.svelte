<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useSavedCue } from '@/components/admin/saved-cue'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import NoticeRow from './NoticeRow.svelte'
  import SaveAction from './SaveAction.svelte'
  import { secretHint } from './secret-hint'
  import { needsKeyFirst, renderSummary, useAgentWebSearch, type AgentWebSearch, type WebBackend } from './search'

  // AGENT WEB SEARCH: the one provider every agent's own Hermes searches
  // with. Picks autosave; a vendor key is the one explicit save (a secret
  // being committed). A keyed vendor with no stored key is held as a draft
  // until its key arrives, because the server refuses the pick without one.
  type Field = 'backend' | 'extractBackend'

  const qc = useQueryClient()
  const autosave = useSavedCue()
  const query = useAgentWebSearch()
  let draft = $state<{ field: Field; name: string } | null>(null)
  let typed = $state<Record<string, string>>({})
  let busy = $state(false)

  const put = async (body: Record<string, unknown>): Promise<boolean> => {
    busy = true
    try {
      qc.setQueryData(['admin-search-agents'], await putJson<AgentWebSearch>('/api/admin/search/agents', body))
      // A key is committed with its own Save. Everything else saved on change.
      if (!('keys' in body)) autosave.flash()
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

<SettingsSection
  title="Agent web search"
  description="The provider every agent searches the web with, picked up at an agent's next restart."
  info="Written into every agent's config as web.search_backend and web.extract_backend, with the provider key in the agent's environment. Research does not use it: research searches with the search endpoint above or a registered search MCP server. Left on the default, Talaria writes nothing and recommends no provider."
>
  <div class="space-y-3">
  <SettingsGroup>
    <QueryState {query} errorTitle="Could not load agent web search" errorVariant="compact">
      {#snippet skeleton()}
        <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
      {/snippet}
      {#snippet children(cfg)}
        <SettingsRow label="Search provider" hint="Where an agent's web searches go." as="label">
          <Select size="sm" value={shown(cfg, 'backend')} disabled={busy} onchange={(e) => pick(cfg, 'backend', e.currentTarget.value)} class="w-[280px]">
            <option value="">Agent default</option>
            {#each cfg.backends as b (b.name)}
              <option value={b.name} disabled={b.name === 'searxng' && !cfg.searchUrl}>{b.label}</option>
            {/each}
          </Select>
        </SettingsRow>
        <SettingsRow label="Page reader" hint="What opens a page and reads its text." as="label">
          <Select size="sm" value={shown(cfg, 'extractBackend')} disabled={busy} onchange={(e) => pick(cfg, 'extractBackend', e.currentTarget.value)} class="w-[280px]">
            <option value="">Agent default</option>
            {#each cfg.backends.filter((b) => b.extract) as b (b.name)}
              <option value={b.name}>{b.label}</option>
            {/each}
          </Select>
        </SettingsRow>
        {#each keyed(cfg) as b (b.name)}
          <div transition:slide={{ duration: 150 }}>
            <SettingsRow
              label="{b.label} key"
              hint={draft?.name === b.name && !typed[b.name]
                ? `${b.label} needs its key before agents can use it.`
                : secretHint(!!cfg.keys[b.name])}
              info={b.sdkOnFirstUse
                ? `Agents install the ${b.label} SDK on first use, and an agent may need a restart before ${b.label} answers.`
                : undefined}
            >
              <Input
                id="aws-key-{b.name}"
                aria-label="{b.label} key"
                size="sm"
                type="password"
                autocomplete="off"
                value={typed[b.name] ?? ''}
                oninput={(e) => (typed = { ...typed, [b.name]: e.currentTarget.value })}
                onkeydown={submitOnEnter(() => typed[b.name] && !busy && void saveKey(b.name))}
                placeholder={cfg.keys[b.name] ? 'Paste a new key' : 'Paste the provider key'}
                class="w-[280px]"
              />
              <SaveAction size="md" dirty={!!typed[b.name]} {busy} onclick={() => void saveKey(b.name)}>Save key</SaveAction>
            </SettingsRow>
          </div>
        {/each}
        <SettingsRow
          label="Free public tiers"
          hint="Let agents fall back to a provider's anonymous free tier."
          info="Off: once a provider or search endpoint is set, agents never fall back to the anonymous free tiers of Exa, Parallel, Firecrawl or Keenable, and a failing provider is not retried through them. On: agents keep that fallback."
        >
          <Toggle
            checked={cfg.allowKeylessPublicTiers}
            disabled={busy}
            onChange={(checked) => void put({ allowKeylessPublicTiers: checked })}
            title="Allow free public tiers"
          />
        </SettingsRow>
        {#each cfg.rendered.warnings as w, i (i)}
          <NoticeRow status="warn">{w.replace(/^agent web search: /, '')}</NoticeRow>
        {/each}
      {/snippet}
    </QueryState>
  </SettingsGroup>
  {#if query.data}
    <Disclosure title="What agents receive" variant="card">
      <p class="break-words font-mono text-xs leading-5 text-muted">{renderSummary(query.data)}</p>
    </Disclosure>
  {/if}
  </div>
</SettingsSection>
