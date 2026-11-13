<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { RefreshCw, Rocket, Pin, PinOff } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
  import { relativeTime, untilTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'

  // THE WORKBENCH HARNESSES: which Oh My Pi and which bun (the harness's
  // own) every new Developer Agent job runs, and the channel that keeps them
  // current (talaria-workbench-updates; docs/UPDATES.md "The workbench
  // harnesses"). Automatic follows upstream — a check every ~6 h, a soak, a
  // smoke test on one workbench, then new jobs — and Pinned holds one
  // version. A running job keeps what it started on. The same shape as the
  // Agent image panel beside it; admin-only on the server.

  interface Candidate {
    version: string
    firstSeenAt: number
    readyAt: number
    requiresBun: string | null
    forced: boolean
  }
  interface HeldView {
    version: string
    reason: string
    at: number
    department: string | null
  }
  interface ToolView {
    tool: 'omp' | 'bun'
    label: string
    source: string
    current: string
    currentSource: 'verified' | 'bootstrap'
    bootstrap: string
    previous: string | null
    pinned: string | null
    candidate: Candidate | null
    next: { version: string; bun: string; omp: string; why: string } | null
    held: HeldView[]
    detail: string | null
    lastCheck: { at: number; latest: string | null; error: string | null } | null
    versions: { version: string; requiresBun: string | null; deprecated: string | null; held: boolean }[]
    requiresBun: string | null
  }
  interface HarnessStatus {
    tools: ToolView[]
    nextCheckAt: number | null
    retryAt: number | null
    minAgeHours: number
    volumes: {
      department: string
      agent?: string
      bun: string | null
      omp: string | null
      at: number
      error: string | null
      refused: { bun: string; omp: string } | null
      hostFailures: number
      retryAt: number | null
    }[]
    events: { at: number; text: string }[]
  }

  const queryClient = useQueryClient()
  const query = createQuery<HarnessStatus>(() => ({
    queryKey: ['workbench-harnesses'],
    queryFn: async () => (await getJson<{ status: HarnessStatus }>('/api/workbench/harnesses')).status,
    refetchInterval: (q) => (q.state.data?.tools.some((t) => t.next) ? 5_000 : 30_000),
  }))
  const s = $derived(query.data)

  const ago = (ms: number | null | undefined) => (ms ? relativeTime(new Date(ms).toISOString()) : 'never')
  const until = untilTime
  const unreadable = $derived(query.isError && /unreadable/.test(errorMessage(query.error)))

  let busy = $state<string | null>(null)
  let line = $state<{ tone: 'ok' | 'danger'; text: string } | null>(null)
  const act = async (action: string, body: Record<string, unknown> = {}) => {
    busy = `${action}:${body.tool ?? ''}`
    line = null
    try {
      const r = await postJson<{ message: string }>('/api/workbench/harnesses', { action, ...body })
      line = { tone: 'ok', text: r.message }
      await queryClient.invalidateQueries({ queryKey: ['workbench-harnesses'] })
    } catch (e) {
      line = { tone: 'danger', text: errorMessage(e) }
    } finally {
      busy = null
    }
  }

  let pinTyped = $state<Record<string, string>>({})
  const pinTo = async (t: ToolView, version: string) => {
    const ok = await confirm({
      title: `Pin ${t.label} to ${version}?`,
      message: `It is installed beside what workbenches run and smoke-tested on one of them; once it passes, every new job runs ${t.label} ${version} until you unpin. A job already running keeps what it started on.`,
      confirmLabel: 'Pin',
    })
    if (ok) await act('pin', { tool: t.tool, version })
  }
  const updateNow = async (t: ToolView) => {
    const v = t.candidate?.version
    const ok = await confirm({
      title: `Update ${t.label} to ${v} now?`,
      message: `It skips the rest of its ${s?.minAgeHours ?? 24} h soak. It is still smoke-tested on a workbench first, and held if it fails; a job already running keeps what it started on.`,
      confirmLabel: 'Update now',
    })
    if (ok) await act('update', { tool: t.tool })
  }
  const allowAgain = async (t: ToolView, h: HeldView) => {
    const ok = await confirm({
      title: `Allow ${t.label} ${h.version} again?`,
      message: `It was held: ${h.reason.split('\n')[0]}. Once allowed, it is smoke-tested again within a minute or two, and adopted if it passes.`,
      confirmLabel: 'Allow again',
    })
    if (ok) await act('allow', { tool: t.tool, version: h.version })
  }
  const reset = async () => {
    const ok = await confirm({
      title: 'Reset the workbench harness state?',
      message: 'Its saved state cannot be read by this version of Talaria, so harness updates are paused (jobs run the compiled versions). Reset keeps it as it is under a new name and starts fresh.',
      confirmLabel: 'Reset',
    })
    if (ok) {
      await act('reset')
      await query.refetch()
    }
  }
  const pinnable = (t: ToolView) => t.versions.filter((v) => !v.held && !v.deprecated && v.version !== t.current)
  // One field takes both: a version picked from the known ones, or any typed.
  const pinTarget = (t: ToolView): string | null => {
    const typed = (pinTyped[t.tool] ?? '').trim()
    return /^\d+\.\d+\.\d+$/.test(typed) || pinnable(t).some((v) => v.version === typed) ? typed : null
  }
  const TONE = { ok: 'text-fg', danger: 'text-danger' } as const
  let showHistory = $state(false)
</script>

<SettingsSection title="Workbench tools">
  {#snippet action()}
    {#if s?.tools.some((t) => t.next)}
      <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="accent" pulse title="An update is being smoke-tested" />Testing an update</span>
    {/if}
  {/snippet}
  <SettingsGroup>
    {#if query.isPending}
      <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Reading the workbench tools</p>
    {:else if query.isError || !s}
      <div class="space-y-2 px-5 py-4 font-sans text-[13px] leading-5">
        <p class="text-danger">Could not read the workbench tools: {errorMessage(query.error)}</p>
        {#if unreadable}
          <Button variant="outline" size="sm" disabled={busy !== null} onclick={reset}>Reset the state</Button>
          {#if line}<p class={TONE[line.tone]}>{line.text}</p>{/if}
        {/if}
      </div>
    {:else}
      {#each s.tools as t (t.tool)}
        <div>
          <SettingsRow label={t.label}>
            {#snippet meta()}
              <code class="font-mono text-xs text-muted">{t.current}</code>
              <Chip>{t.pinned ? `Pinned to ${t.pinned}` : 'Automatic'}</Chip>
            {/snippet}
            {#snippet hint()}
              {#if t.lastCheck}
                Checked {ago(t.lastCheck.at)}{#if t.lastCheck.latest}, newest is <span class="font-mono text-xs">{t.lastCheck.latest}</span>{/if}.
              {:else}
                Not checked yet.
              {/if}
              {#if !t.pinned && t.currentSource === 'bootstrap'}The built-in version runs until one is verified here.{/if}
              {#if t.tool === 'omp' && t.requiresBun}Needs bun <span class="font-mono text-xs">{t.requiresBun}</span>.{/if}
              {#if t.previous && t.previous !== t.current}Falls back to <span class="font-mono text-xs">{t.previous}</span>.{/if}
              {#if t.lastCheck?.error}<span class="block text-danger">The last check failed: {t.lastCheck.error}</span>{/if}
              {#if t.next}
                <span class="flex items-center gap-2 text-fg">
                  <StatusDot status="accent" pulse title="Being smoke-tested" />Testing Oh My Pi {t.next.omp} on bun {t.next.bun}: {t.next.why}
                </span>
              {:else if t.detail}
                <span class="block">{t.detail}</span>
              {/if}
            {/snippet}
            {#if t.candidate && !t.pinned}
              <Button variant="outline" size="sm" disabled={busy !== null || t.candidate.forced} onclick={() => updateNow(t)}>
                <Rocket size={14} />
                Update to {t.candidate.version}
              </Button>
            {/if}
            {#if t.pinned}
              <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('unpin', { tool: t.tool })}>
                <PinOff size={14} />
                Unpin
              </Button>
            {:else}
              <Input
                size="sm"
                class="w-28 font-mono placeholder:font-sans"
                list="pin-versions-{t.tool}"
                bind:value={pinTyped[t.tool]}
                placeholder="Version"
                aria-label="{t.label} version to pin"
              />
              <datalist id="pin-versions-{t.tool}">
                {#each pinnable(t) as v (v.version)}
                  <option value={v.version}></option>
                {/each}
              </datalist>
              <Button variant="outline" size="md" disabled={busy !== null || !pinTarget(t)} onclick={() => pinTo(t, pinTarget(t)!)}>
                <Pin size={14} />
                Pin
              </Button>
            {/if}
          </SettingsRow>
          {#each t.held as h (h.version)}
            <div class="flex items-center justify-between gap-6 px-5 pb-3 font-sans text-[13px] leading-5">
              <span class="min-w-0">
                <span class="text-danger">Held <span class="font-mono text-xs">{h.version}</span></span>
                <span class="text-muted"> · {h.reason.split('\n')[0]}</span>
              </span>
              <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => allowAgain(t, h)}>Allow again</Button>
            </div>
          {/each}
        </div>
      {/each}

      <SettingsRow label="Updates">
        {#snippet hint()}
          {#if until(s.nextCheckAt)}Next check {until(s.nextCheckAt)}.{/if}
          A new release waits {s.minAgeHours} h first.
          {#if until(s.retryAt)}The host failed the last test and is tried again {until(s.retryAt)}.{/if}
          {#if line}<span transition:slide={{ duration: 150 }} class="block {TONE[line.tone]}">{line.text}</span>{/if}
        {/snippet}
        <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('check')}>
          <RefreshCw size={14} />
          {busy === 'check:' ? 'Checking' : 'Check now'}
        </Button>
      </SettingsRow>

      {#if s.volumes.length > 0}
        <div class="px-5 py-4">
          <div class="mb-1 font-sans text-xs font-medium leading-4 text-muted">Installed per agent</div>
          <ul>
            {#each s.volumes as v (v.department)}
              <li class="flex min-h-8 items-center justify-between gap-6 font-sans text-[13px] leading-5">
                <span class="min-w-0 truncate text-fg">{v.agent ?? v.department}</span>
                <span class="shrink-0 {v.refused || v.retryAt ? 'text-danger' : 'text-muted'}">
                  {#if v.refused}would not take omp {v.refused.omp} · bun {v.refused.bun}: {(v.error ?? '').split('\n')[0]}. Its jobs run the fallback.
                  {:else if v.retryAt}its host failed {v.hostFailures}×, tried again {until(v.retryAt)}
                  {:else}<span class="font-mono text-xs">omp {v.omp} · bun {v.bun}</span>{/if}
                </span>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if s.events.length > 0}
        <div>
          <SettingsRow
            label="History"
            hint={`${s.events.length} recorded ${s.events.length === 1 ? 'change' : 'changes'}.`}
            onclick={() => (showHistory = !showHistory)}
            expanded={showHistory}
          />
          {#if showHistory}
            <ul transition:slide={{ duration: 150 }} class="space-y-1 px-5 pb-4 font-sans text-[13px] leading-5 text-muted">
              {#each s.events as ev, n (n)}
                <li><span class="text-fg">{ago(ev.at)}</span> · {ev.text}</li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    {/if}
  </SettingsGroup>
</SettingsSection>
