<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
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
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
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
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
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
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    floor = null
    floorFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }
</script>

<Panel>
  <SectionHeader class="mb-2" title="Settings" />
  {#if query.isPending}
    <!-- Hold the retention row so the input never renders blank, then fills. -->
    <div class="flex items-center gap-3">
      <div class="min-w-0 flex-1 space-y-1.5">
        <Skeleton class="h-3 w-28 rounded-full" />
        <Skeleton class="h-2.5 w-64 rounded-full" />
      </div>
      <Skeleton class="h-8 w-24 shrink-0" />
      <Skeleton class="h-7 w-16 shrink-0" />
    </div>
  {:else if !data}
    <!-- An empty retention box reads as "0 = keep forever". Saving from there
         would silently change the org's audit policy. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your settings"
      onRetry={() => void query.refetch()}
    />
  {:else}
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
  {/if}
</Panel>
