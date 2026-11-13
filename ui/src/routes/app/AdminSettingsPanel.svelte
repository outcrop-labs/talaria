<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { sameToolsets, toggleToolset, toolsetRows } from '@/components/fleet/toolsets'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useAdminSettings } from './admin'

  const qc = useQueryClient()
  const query = useAdminSettings()
  const data = $derived(query.data)
  let days = $state('')
  const auditFlash = useSavedFlash()
  const value = $derived(days !== '' ? days : String(data?.auditRetentionDays ?? ''))
  const save = async () => {
    const n = Number(value)
    if (!Number.isFinite(n)) return
    try {
      await putJson<{ ok: true }>('/api/admin/settings', { auditRetentionDays: n })
    } catch (e) {
      // Leave the typed value in place so the failed edit is retryable.
      toastError('Save failed', e)
      return
    }
    days = ''
    auditFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }

  // ── LLM budgets ────────────────────────────────────────────────────────────
  // Blank = unlimited for every cap, and unlimited is the default an upgraded
  // deployment keeps. Edits are string state over the loaded numbers (null =
  // "not touched yet"), the AdminOrgPanel pattern.
  const savedFlash = useSavedFlash()
  let win = $state<string | null>(null)
  let orgTokens = $state<string | null>(null)
  let orgUsd = $state<string | null>(null)
  let perTokens = $state<string | null>(null)
  let perUsd = $state<string | null>(null)
  const b = $derived(data?.llmBudgets)
  const capVal = (v: string | null, loaded: number | null | undefined) =>
    v ?? (typeof loaded === 'number' && loaded > 0 ? String(loaded) : '')
  const winVal = $derived(win ?? String(b?.windowHours ?? 24))
  const orgTokensVal = $derived(capVal(orgTokens, b?.org?.tokens))
  const orgUsdVal = $derived(capVal(orgUsd, b?.org?.usd))
  const perTokensVal = $derived(capVal(perTokens, b?.perAgent?.tokens))
  const perUsdVal = $derived(capVal(perUsd, b?.perAgent?.usd))
  const caps = (tokens: string, usd: string) => ({
    tokens: tokens.trim() === '' ? null : Number(tokens),
    usd: usd.trim() === '' ? null : Number(usd),
  })
  const numeric = (...vals: Array<string | null>) => vals.every((v) => v === null || v.trim() === '' || Number.isFinite(Number(v)))
  const budgetsValid = $derived(numeric(orgTokens, orgUsd, perTokens, perUsd) && Number.isFinite(Number(winVal)) && Number(winVal) >= 1)
  const budgetsDirty = $derived(
    !!b &&
      (orgTokensVal !== capVal(null, b.org?.tokens) ||
        orgUsdVal !== capVal(null, b.org?.usd) ||
        perTokensVal !== capVal(null, b.perAgent?.tokens) ||
        perUsdVal !== capVal(null, b.perAgent?.usd) ||
        Number(winVal) !== b.windowHours),
  )
  // Which row says Saved: the one whose field was just left.
  let budgetField = $state<'win' | 'org' | 'per' | null>(null)
  const commitBudgets = async (field: 'win' | 'org' | 'per') => {
    if (!budgetsValid || !budgetsDirty) return
    budgetField = field
    await saveBudgets()
  }
  const saveBudgets = async () => {
    try {
      await putJson<{ ok: true }>('/api/admin/settings', {
        // The panel edits the two shared scopes; per-caller overrides live in
        // the API and must ride along untouched or this save would erase them.
        llmBudgets: {
          windowHours: Number(winVal),
          org: caps(orgTokensVal, orgUsdVal),
          perAgent: caps(perTokensVal, perUsdVal),
          agents: b?.agents ?? {},
        },
      })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    win = orgTokens = orgUsd = perTokens = perUsd = null
    savedFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }

  let floor = $state<string | null>(null)
  const floorVal = $derived(floor ?? String(data?.cronMinIntervalMinutes ?? 5))
  // Its own flash, so saving the floor doesn't light "Saved" next to the
  // budgets button (and vice versa).
  const floorFlash = useSavedFlash()
  const saveFloor = async () => {
    const n = Number(floorVal)
    if (!Number.isFinite(n) || n < 0) return
    try {
      await putJson<{ ok: true }>('/api/admin/settings', { cronMinIntervalMinutes: n })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    floor = null
    floorFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }

  // ── Agent toolsets: the org-wide floor ─────────────────────────────────────
  // The same grid grammar as the agent editor (a box is the toolset being
  // AVAILABLE; clearing one closes it), over the same logic in
  // components/fleet/toolsets.ts, so the two surfaces cannot disagree about
  // the sign. Here the stored list is the floor itself: what is closed for
  // every agent. Two kinds of row are locked and say why: the pair Talaria
  // replaces with its own tools, and names the api's environment pins.
  // Explicit Save, not autosave: this is a governance control that lands on
  // every agent at once — saving rewrites every agent's configuration (it
  // restarts nobody), and each running agent reads the floor on its next turn.
  const vocab = $derived(data?.toolsets)
  const envClosed = $derived(vocab?.env ?? [])
  const locked = $derived([...(vocab?.builtin ?? []), ...envClosed])
  // null = untouched, showing the stored floor. A list = the pending edit.
  let floorEdit = $state<string[] | null>(null)
  let floorSaving = $state(false)
  const toolsetFlash = useSavedFlash()
  const closedShown = $derived(floorEdit ?? data?.mandatoryDisabledToolsets ?? [])
  const floorRows = $derived(vocab ? toolsetRows(vocab.governed, locked, closedShown, envClosed) : [])
  const floorDirty = $derived(floorEdit !== null && !sameToolsets(floorEdit, data?.mandatoryDisabledToolsets ?? []))
  const setFloor = (name: string, on: boolean) => {
    // Only the builtin pair is stripped from what gets stored. An env-pinned
    // name stays in the row if it is there (its box is disabled, so this
    // cannot add or remove it), so lifting the env pin later leaves the
    // stored floor exactly as the admin last saved it.
    floorEdit = toggleToolset(closedShown, name, on, vocab?.builtin ?? [])
  }
  const saveToolsetFloor = async () => {
    if (!floorEdit) return
    floorSaving = true
    try {
      await putJson<{ ok: true }>('/api/admin/settings', { mandatoryDisabledToolsets: floorEdit })
    } catch (e) {
      // The pending boxes stay, so a refused name can be fixed rather than
      // re-ticked.
      toastError('Save failed', e)
      return
    } finally {
      floorSaving = false
    }
    floorEdit = null
    toolsetFlash.flash()
    // The agent editor locks its rows off the roster's floor, so it must see
    // this too.
    await Promise.all([qc.invalidateQueries({ queryKey: ['admin-settings'] }), qc.invalidateQueries({ queryKey: ['fleet-defs'] })])
  }
</script>

{#snippet saved(on: boolean)}
  {#if on}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
{/snippet}
{#snippet unit(word: string, width = 'w-14')}
  <span class="{width} shrink-0 font-sans text-[13px] leading-5 text-muted">{word}</span>
{/snippet}

<SettingsSection
  title="Limits"
  description="Ceilings that apply to every agent, API key and built-in feature."
  info="A call is refused once the spend recorded in the window reaches a limit, and it resumes as older spend ages out. A dollar limit ignores models with no price set, which the Models page lists. A token limit counts everything."
>
  <SettingsGroup>
    <!-- An empty retention box reads as "0 = keep forever". Saving from there
         would silently change the org's audit policy, so a failed read must
         never reach the form — QueryState's error branch owns that. -->
    <QueryState query={query} errorTitle="Could not load your settings" errorVariant="compact">
      {#snippet skeleton()}
        {#each ['Spending window', 'Whole organization', 'Each agent or key'] as label (label)}
          <SettingsRow {label}><Skeleton class="h-9 w-[164px]" /></SettingsRow>
        {/each}
      {/snippet}
      {#snippet children(data)}
        <SettingsRow label="Spending window" hint="How many hours of spend each limit counts.">
          {@render saved(savedFlash.saved && budgetField === 'win')}
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="1"
              aria-label="Spending window in hours"
              value={winVal}
              oninput={(e) => (win = e.currentTarget.value)}
              onblur={() => void commitBudgets('win')}
              onkeydown={inlineEditKeys(() => (win = null))}
              class="w-[100px]"
            />
            {@render unit('hours')}
          </span>
        </SettingsRow>
        <SettingsRow label="Whole organization" hint="A limit in tokens, in dollars, or both. Empty means no limit.">
          {@render saved(savedFlash.saved && budgetField === 'org')}
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="0"
              aria-label="Organization token limit"
              placeholder="No limit"
              value={orgTokensVal}
              oninput={(e) => (orgTokens = e.currentTarget.value)}
              onblur={() => void commitBudgets('org')}
              onkeydown={inlineEditKeys(() => (orgTokens = null))}
              class="w-[136px]"
            />
            {@render unit('tokens', 'w-12')}
          </span>
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="0"
              aria-label="Organization dollar limit"
              placeholder="No limit"
              value={orgUsdVal}
              oninput={(e) => (orgUsd = e.currentTarget.value)}
              onblur={() => void commitBudgets('org')}
              onkeydown={inlineEditKeys(() => (orgUsd = null))}
              class="w-[100px]"
            />
            {@render unit('dollars')}
          </span>
        </SettingsRow>
        <SettingsRow label="Each agent or key" hint="The same limits, counted for one agent or API key at a time.">
          {@render saved(savedFlash.saved && budgetField === 'per')}
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="0"
              aria-label="Token limit for each agent or key"
              placeholder="No limit"
              value={perTokensVal}
              oninput={(e) => (perTokens = e.currentTarget.value)}
              onblur={() => void commitBudgets('per')}
              onkeydown={inlineEditKeys(() => (perTokens = null))}
              class="w-[136px]"
            />
            {@render unit('tokens', 'w-12')}
          </span>
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="0"
              aria-label="Dollar limit for each agent or key"
              placeholder="No limit"
              value={perUsdVal}
              oninput={(e) => (perUsd = e.currentTarget.value)}
              onblur={() => void commitBudgets('per')}
              onkeydown={inlineEditKeys(() => (perUsd = null))}
              class="w-[100px]"
            />
            {@render unit('dollars')}
          </span>
        </SettingsRow>
        <SettingsRow
          label="Fastest schedule"
          hint="The shortest gap allowed between an agent's scheduled runs. 0 means no limit."
        >
          {@render saved(floorFlash.saved)}
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              min="0"
              aria-label="Fastest schedule in minutes"
              value={floorVal}
              oninput={(e) => (floor = e.currentTarget.value)}
              onblur={() => floor !== null && Number(floor) !== data.cronMinIntervalMinutes && void saveFloor()}
              onkeydown={inlineEditKeys(() => (floor = null))}
              class="w-[100px]"
            />
            {@render unit('minutes')}
          </span>
        </SettingsRow>
        <SettingsRow label="Audit log" hint="How many days of history to keep. 0 keeps it forever.">
          {@render saved(auditFlash.saved)}
          <span class="flex items-center gap-2">
            <Input
              size="sm"
              type="number"
              aria-label="Audit log retention in days"
              {value}
              oninput={(e) => (days = e.currentTarget.value)}
              onblur={() => days !== '' && Number(days) !== data.auditRetentionDays && void save()}
              onkeydown={inlineEditKeys(() => (days = ''))}
              class="w-[100px]"
            />
            {@render unit('days')}
          </span>
        </SettingsRow>
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>

{#if floorRows.length}
  <SettingsSection
    title="Agent tools"
    description="The tool groups any agent may use. Turn one off and no agent can use it."
    info="A group turned off here shows as locked in each agent's own editor. Talaria's own to-do and clarify tools replace two built-in ones, so those two stay off. Saving restarts nobody: each running agent picks up the change on its next turn."
  >
    <SettingsGroup>
      <div class="grid grid-cols-2 gap-x-6 gap-y-3 px-5 py-4 sm:grid-cols-3 md:grid-cols-4">
        {#each floorRows as r (r.name)}
          <Checkbox
            checked={r.on}
            disabled={r.forced || floorSaving}
            onChange={(on) => setFloor(r.name, on)}
            label={r.name}
            title={r.orgClosed
              ? `${r.name} is kept off by TALARIA_MANDATORY_DISABLED_TOOLSETS in the server environment`
              : r.forced
                ? `Talaria's own ${r.name} tool is the one a person sees, so the built-in one stays off for every agent`
                : r.on
                  ? `${r.name} is available to agents. Clear it to turn it off for all of them`
                  : `${r.name} is off for every agent`}
            class={r.on ? 'gap-2 font-mono text-fg' : 'gap-2 font-mono'}
          />
        {/each}
      </div>
      <div class="flex items-center gap-3 px-5 py-3">
        <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
          {#if toolsetFlash.saved}
            <span class="text-success">Saved. Agents pick it up on their next turn.</span>
          {:else if envClosed.length}
            Kept off by the server environment: <span class="font-mono text-xs">{envClosed.join(', ')}</span>
          {/if}
        </span>
        {#if floorDirty}
          <Button size="sm" variant="ghost" disabled={floorSaving} onclick={() => (floorEdit = null)}>Reset</Button>
        {/if}
        <Button size="sm" variant="outline" onclick={() => void saveToolsetFloor()} disabled={!floorDirty || floorSaving}>
          Save tools
        </Button>
      </div>
    </SettingsGroup>
  </SettingsSection>
{/if}
