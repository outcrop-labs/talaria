<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { RefreshCw, Rocket, Pin, PinOff } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Select from '@/components/ui/Select.svelte'
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
    volumes: { department: string; agent?: string; bun: string | null; omp: string | null; at: number; error: string | null }[]
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

  let pinChoice = $state<Record<string, string>>({})
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
  const TONE = { ok: 'text-fg', danger: 'text-danger' } as const
  let showHistory = $state(false)
</script>

<Panel class="px-4 py-3">
  <div class="mb-1 flex min-h-6 items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Workbench harnesses</span>
    {#if s?.tools.some((t) => t.next)}<StatusDot status="accent" pulse title="An update is being smoke-tested" />{/if}
  </div>
  {#if query.isPending}
    <p class="py-2 font-sans text-xs text-muted">Reading the workbench harnesses…</p>
  {:else if query.isError || !s}
    <p class="py-2 font-sans text-xs text-danger">Could not read the workbench harnesses: {errorMessage(query.error)}</p>
    {#if unreadable}
      <Button variant="outline" size="sm" disabled={busy !== null} onclick={reset}>Reset the state</Button>
      {#if line}<p class="pt-2 font-sans text-xs {TONE[line.tone]}">{line.text}</p>{/if}
    {/if}
  {:else}
    <div class="space-y-3 py-2">
      {#each s.tools as t (t.tool)}
        <div class="space-y-1">
          <div class="font-sans text-sm text-fg">
            {t.label} <span class="font-mono">{t.current}</span>
            <span class="text-muted">·</span>
            {#if t.pinned}
              Pinned to <span class="font-mono">{t.pinned}</span>
            {:else}
              Automatic{t.currentSource === 'bootstrap' ? ' (the compiled version until one is verified here)' : ''}
            {/if}
          </div>
          <div class="font-sans text-xs text-muted">
            {#if t.lastCheck}
              Checked {ago(t.lastCheck.at)}{#if t.lastCheck.latest}: newest is <span class="font-mono">{t.lastCheck.latest}</span>{/if}.
            {:else}
              Not checked yet.
            {/if}
            {#if t.tool === 'omp' && t.requiresBun}Needs bun <span class="font-mono">{t.requiresBun}</span>.{/if}
            {#if t.previous && t.previous !== t.current}Falls back to <span class="font-mono">{t.previous}</span>.{/if}
          </div>
          {#if t.lastCheck?.error}
            <div class="font-sans text-xs text-danger">The last check failed: {t.lastCheck.error}</div>
          {/if}
          {#if t.next}
            <div class="flex items-center gap-2 font-sans text-xs text-fg">
              <StatusDot status="accent" pulse title="Being smoke-tested" /> Smoke-testing Oh My Pi {t.next.omp} on bun {t.next.bun} — {t.next.why}
            </div>
          {:else if t.detail}
            <div class="font-sans text-xs text-muted">{t.detail}</div>
          {/if}
          {#each t.held as h (h.version)}
            <div class="flex items-baseline justify-between gap-3">
              <span class="min-w-0 font-sans text-xs">
                <span class="text-danger">Held <span class="font-mono">{h.version}</span></span>
                <span class="text-muted"> — {h.reason.split('\n')[0]}</span>
              </span>
              <Button variant="ghost" size="sm" disabled={busy !== null} onclick={() => allowAgain(t, h)}>Allow again</Button>
            </div>
          {/each}
          <div class="flex flex-wrap items-center gap-2">
            {#if t.candidate && !t.pinned}
              <Button variant="outline" size="sm" disabled={busy !== null || t.candidate.forced} onclick={() => updateNow(t)}>
                <Rocket size={14} />
                Update to {t.candidate.version} now
              </Button>
            {/if}
            {#if t.pinned}
              <Button variant="ghost" size="sm" disabled={busy !== null} onclick={() => act('unpin', { tool: t.tool })}>
                <PinOff size={14} />
                Unpin: follow upstream
              </Button>
            {:else}
              {#if pinnable(t).length > 0}
                <Select size="sm" bind:value={pinChoice[t.tool]} aria-label="{t.label} version to pin">
                  {#each pinnable(t) as v (v.version)}
                    <option value={v.version}>{v.version}</option>
                  {/each}
                </Select>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy !== null || !(pinChoice[t.tool] ?? pinnable(t)[0]?.version)}
                  onclick={() => pinTo(t, pinChoice[t.tool] ?? pinnable(t)[0]!.version)}
                >
                  <Pin size={14} />
                  Pin
                </Button>
              {/if}
              <Input size="sm" class="w-24 font-[var(--font-mono)]" bind:value={pinTyped[t.tool]} placeholder={t.current} aria-label="Or pin a {t.label} version" />
              <Button
                variant="ghost"
                size="sm"
                disabled={busy !== null || !/^\d+\.\d+\.\d+$/.test((pinTyped[t.tool] ?? '').trim())}
                onclick={() => pinTo(t, (pinTyped[t.tool] ?? '').trim())}
              >
                Pin version
              </Button>
            {/if}
          </div>
        </div>
      {/each}

      <div class="flex flex-wrap items-center gap-2 border-t border-line pt-3">
        <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('check')}>
          <RefreshCw size={14} />
          {busy === 'check:' ? 'Checking…' : 'Check now'}
        </Button>
        <span class="font-sans text-xs text-muted">
          {#if until(s.nextCheckAt)}Next check {until(s.nextCheckAt)}.{/if}
          A new release soaks {s.minAgeHours} h first.
          {#if until(s.retryAt)}The host failed the last smoke test; it is tried again {until(s.retryAt)}.{/if}
        </span>
      </div>

      {#if s.volumes.length > 0}
        <ul class="divide-y divide-line-subtle">
          {#each s.volumes as v (v.department)}
            <li class="flex items-baseline justify-between gap-3 py-1.5">
              <span class="min-w-0 truncate font-sans text-sm text-fg">{v.agent ?? v.department}</span>
              <span class="shrink-0 font-sans text-xs {v.error ? 'text-danger' : 'text-muted'}">
                {#if v.error}would not take it: {v.error.split('\n')[0]}{:else}<span class="font-mono">omp {v.omp} · bun {v.bun}</span>{/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}

      {#if s.events.length > 0}
        <div class="border-t border-line pt-2">
          <Button variant="ghost" size="sm" onclick={() => (showHistory = !showHistory)}>
            {showHistory ? 'Hide history' : `History (${s.events.length})`}
          </Button>
          {#if showHistory}
            <ul transition:slide={{ duration: 150 }} class="mt-1 space-y-1">
              {#each s.events as ev, n (n)}
                <li class="font-sans text-xs text-muted"><span class="text-fg">{ago(ev.at)}</span> — {ev.text}</li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
      {#if line}
        <p transition:slide={{ duration: 150 }} class="font-sans text-xs {TONE[line.tone]}">{line.text}</p>
      {/if}
    </div>
  {/if}
</Panel>
