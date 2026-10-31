<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { sameToolsets, toggleToolset, toolsetRows } from '@/components/fleet/toolsets'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useAdminSettings } from './admin'

  const qc = useQueryClient()
  const query = useAdminSettings()
  const data = $derived(query.data)
  let days = $state('')
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
  // every agent at once — saving renders the fleet (it restarts nobody), and
  // each running agent reads the floor on its next turn.
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

<Panel>
  <SectionHeader class="mb-2" title="Settings" />
  <QueryState query={query} errorTitle="Could not load your settings" errorVariant="compact">
    {#snippet skeleton()}
      <!-- Hold the retention row so the input never renders blank, then fills. -->
      <div class="flex items-center gap-3">
        <div class="min-w-0 flex-1 space-y-1.5">
          <Skeleton class="h-3 w-28 rounded-full" />
          <Skeleton class="h-2.5 w-64 rounded-full" />
        </div>
        <Skeleton class="h-8 w-24 shrink-0" />
        <Skeleton class="h-7 w-16 shrink-0" />
      </div>
    {/snippet}
    {#snippet children(data)}
    <!-- An empty retention box reads as "0 = keep forever". Saving from there
         would silently change the org's audit policy, so a failed read must
         never reach the form — QueryState's error branch owns that. -->
    <div class="flex items-center gap-3">
      <div class="min-w-0 flex-1">
        <div class="text-sm text-fg">Audit retention</div>
        <div class="text-xs text-muted">How many days to keep the audit log. 0 = keep forever.</div>
      </div>
      <Input
        size="sm"
        type="number"
        {value}
        oninput={(e) => (days = e.currentTarget.value)}
        onkeydown={submitOnEnter(() => days !== '' && Number(days) !== data?.auditRetentionDays && void save())}
        class="w-24 shrink-0"
      />
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">days</span>
      <Button size="sm" class="shrink-0" onclick={() => void save()} disabled={days === '' || Number(days) === data?.auditRetentionDays}>
        Save
      </Button>
    </div>

    <SectionHeader class="mt-6 mb-2" title="LLM budgets" />
    <p class="mb-3 text-xs text-muted">
      Rolling-window spend ceilings, checked before every gateway call — agent turns, API keys, and internal features
      alike. Blank means unlimited, which is the default; a ceiling only refuses once the window's recorded spend has
      reached it. A $ ceiling is never tripped by tokens with no price configured (the Models page shows those);
      a token ceiling bounds everything.
    </p>
    <div class="mb-3 flex flex-wrap items-center gap-3 text-xs">
      <label class="flex items-center gap-2">
        <span class="text-muted">Window</span>
        <Input size="sm" type="number" min="1" value={winVal} oninput={(e) => (win = e.currentTarget.value)} class="w-20" />
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">hours</span>
      </label>
    </div>
    <div class="mb-3 flex flex-wrap items-center gap-3 text-xs">
      <span class="w-20 shrink-0 text-muted">Org-wide</span>
      <label class="flex items-center gap-2">
        <Input size="sm" type="number" min="0" placeholder="tokens" value={orgTokensVal} oninput={(e) => (orgTokens = e.currentTarget.value)} class="w-32" />
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">tokens</span>
      </label>
      <span class="text-muted">or</span>
      <label class="flex items-center gap-2">
        <span class="text-muted">$</span>
        <Input size="sm" type="number" min="0" placeholder="unlimited" value={orgUsdVal} oninput={(e) => (orgUsd = e.currentTarget.value)} class="w-24" />
      </label>
    </div>
    <div class="mb-4 flex flex-wrap items-center gap-3 text-xs">
      <span class="w-20 shrink-0 text-muted">Per caller</span>
      <label class="flex items-center gap-2">
        <Input size="sm" type="number" min="0" placeholder="tokens" value={perTokensVal} oninput={(e) => (perTokens = e.currentTarget.value)} class="w-32" />
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">tokens</span>
      </label>
      <span class="text-muted">or</span>
      <label class="flex items-center gap-2">
        <span class="text-muted">$</span>
        <Input size="sm" type="number" min="0" placeholder="unlimited" value={perUsdVal} oninput={(e) => (perUsd = e.currentTarget.value)} class="w-24" />
      </label>
    </div>
    <div class="flex items-center gap-3">
      <Button size="sm" onclick={() => void saveBudgets()} disabled={!budgetsValid || !budgetsDirty}>
        Save
      </Button>
      {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
      <span class="text-xs text-muted">Whichever ceiling is reached first refuses the call (429, spend resumes as it ages out).</span>
    </div>

    <div class="mt-6 flex items-center gap-3 border-t border-border pt-4">
      <div class="min-w-0 flex-1">
        <div class="text-sm text-fg">Cron minimum interval</div>
        <div class="text-xs text-muted">
          Fastest schedule an agent's cron may be given — a cron is an agent turn is spend. 0 = no floor.
        </div>
      </div>
      <Input
        size="sm"
        type="number"
        min="0"
        value={floorVal}
        oninput={(e) => (floor = e.currentTarget.value)}
        onkeydown={submitOnEnter(() => floor !== null && Number(floor) !== data?.cronMinIntervalMinutes && void saveFloor())}
        class="w-24 shrink-0"
      />
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">min</span>
      <Button size="sm" class="shrink-0" onclick={() => void saveFloor()} disabled={floor === null || Number(floor) === data?.cronMinIntervalMinutes}>
        Save
      </Button>
      {#if floorFlash.saved}<span class="shrink-0 text-xs text-success">Saved</span>{/if}
    </div>

    {#if floorRows.length}
      <div class="mt-6 border-t border-border pt-4">
        <SectionHeader class="mb-2" title="Agent toolsets" />
        <p class="mb-3 text-xs text-muted">
          The Hermes toolsets any agent may carry. A box is the toolset being available to the fleet; clearing one
          closes it for every agent, whatever its managers opened, and a manager sees it locked in the agent's own
          editor. Talaria's own todo and clarify tools replace Hermes' for good, so those two stay closed. Saving
          restarts nobody: the floor reaches every running agent on its next turn.
        </p>
        <div class="grid grid-cols-2 gap-x-4 gap-y-1.5 sm:grid-cols-3 md:grid-cols-4">
          {#each floorRows as r (r.name)}
            <Checkbox
              checked={r.on}
              disabled={r.forced || floorSaving}
              onChange={(on) => setFloor(r.name, on)}
              label={r.name}
              title={r.orgClosed
                ? `${r.name} is pinned closed by TALARIA_MANDATORY_DISABLED_TOOLSETS in the api's environment`
                : r.forced
                  ? `Talaria's own ${r.name} tool is the one a person sees, so Hermes' stays closed for every agent`
                  : r.on
                    ? `${r.name} is available to agents; clear it to close it org-wide`
                    : `${r.name} is closed for every agent`}
              class={r.on ? 'text-fg' : undefined}
            />
          {/each}
        </div>
        <div class="mt-3 flex items-center gap-3">
          <Button size="sm" onclick={() => void saveToolsetFloor()} disabled={!floorDirty || floorSaving}>
            Save
          </Button>
          {#if floorDirty}
            <Button size="sm" variant="ghost" disabled={floorSaving} onclick={() => (floorEdit = null)}>Reset</Button>
          {/if}
          {#if toolsetFlash.saved}<span class="text-xs text-success">Saved. Reaches every agent on its next turn.</span>{/if}
          {#if envClosed.length}
            <span class="text-xs text-muted">Pinned by the api's environment: {envClosed.join(', ')}.</span>
          {/if}
        </div>
      </div>
    {/if}
    {/snippet}
  </QueryState>
</Panel>
