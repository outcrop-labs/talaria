<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { RefreshCw, Rocket, Undo2, Pin, PinOff } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
  import { relativeTime, untilTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'

  // THE AGENT IMAGE: which Hermes every agent runs, and the stable channel
  // that moves them (talaria-hermes-channel; docs/ARCHITECTURE.md "The
  // fleet"). Automatic follows Hermes' `stable` — a check every ~6 h, the new
  // image prepared in the background, a canary, then agents one at a time in
  // the maintenance window — and Pinned holds every agent on one version.
  // Admin-only on the server, so the overview mounts it for admins only.

  interface Labelled {
    digest: string
    label: string
  }
  interface Skipped {
    slug: string
    why: string
    at: number
  }
  interface RolloutView {
    digest: string
    label: string
    fromLabel: string | null
    kind: 'upgrade' | 'rollback' | 'pin' | 'catchup'
    phase: 'cancelled' | 'preparing' | 'waiting' | 'canary' | 'rolling' | 'soaking' | 'done' | 'failed'
    open: boolean
    forced: boolean
    startedAt: number
    updatedAt: number
    agent: string | null
    moved: string[]
    skipped: Skipped[]
    deferred: string[]
    strikes: number
    detail: string | null
  }
  interface ImageView {
    digest: string
    label: string
    configVersion: number | null
    status: 'candidate' | 'good' | 'bad'
    reason: string | null
    goodSince: number | null
    pinnable: boolean
    held: boolean
    contractProblems: string[]
  }
  interface AgentView {
    slug: string
    displayName: string
    image: Labelled | null
    onChannel: boolean
  }
  interface ChannelStatus {
    standing: 'following' | 'pinned' | 'off'
    offReason: string | null
    channel: string
    pinned: Labelled | null
    current: Labelled
    previous: Labelled | null
    compiled: Labelled
    lastCheck: { at: number; digest: string | null; label: string | null; error: string | null } | null
    nextCheckAt: number | null
    window: { start: string; end: string; zone: string; zoneSaid: string; zoneSet: boolean; open: boolean }
    minStableHours: number
    rollout: RolloutView | null
    agents: AgentView[]
    images: ImageView[]
    events: { at: number; text: string }[]
  }

  const queryClient = useQueryClient()
  const query = createQuery<ChannelStatus>(() => ({
    queryKey: ['fleet-hermes'],
    queryFn: async () => (await getJson<{ status: ChannelStatus }>('/api/fleet/hermes')).status,
    refetchInterval: (q) => (q.state.data?.rollout?.open ? 5_000 : 30_000),
  }))
  const s = $derived(query.data)

  const ago = (ms: number | null | undefined) => (ms ? relativeTime(new Date(ms).toISOString()) : 'never')
  const until = untilTime

  const PHASE: Record<RolloutView['phase'], string> = {
    preparing: 'Waiting its time on stable, then downloading and inspecting',
    waiting: 'Ready, waiting',
    canary: 'Testing in a canary',
    rolling: 'Moving agents one at a time',
    soaking: 'Watching every moved agent before calling it good',
    done: 'Done',
    failed: 'Stopped',
    cancelled: 'Not rolled out',
  }
  // The rollout's heading: what it is doing while it runs, what it did once
  // it ended.
  const KIND: Record<RolloutView['kind'], { open: string; done: string; failed: string }> = {
    upgrade: { open: 'Updating to', done: 'Updated to', failed: 'Stopped the update to' },
    rollback: { open: 'Rolling back to', done: 'Rolled back to', failed: 'Stopped the rollback to' },
    pin: { open: 'Pinning to', done: 'Pinned to', failed: 'Stopped pinning to' },
    catchup: { open: 'Catching up to', done: 'Caught up to', failed: 'Stopped catching up to' },
  }
  const heading = (r: RolloutView) =>
    r.phase === 'cancelled'
      ? 'Did not roll out'
      : r.phase === 'failed'
        ? KIND[r.kind].failed
        : r.open
          ? KIND[r.kind].open
          : KIND[r.kind].done

  const rollout = $derived(s?.rollout ?? null)
  // Stable names something the fleet does not run, and it has not failed here.
  const stableNewer = $derived.by(() => {
    const d = s?.lastCheck?.digest
    if (!s || s.standing !== 'following' || !d || d === s.current.digest) return false
    return s.images.find((i) => i.digest === d)?.status !== 'bad'
  })
  // Any version this instance has inspected and not banned; one that never
  // ran here is canaried first. A version tag that was never seen can be
  // typed instead (it is resolved, pulled and canaried).
  const pinnable = $derived((s?.images ?? []).filter((i) => i.pinnable))
  // The open rollout's image, when the contract checks hold it for an admin.
  const held = $derived.by(() => {
    if (!rollout?.open) return null
    const i = s?.images.find((x) => x.digest === rollout.digest)
    return i?.held ? i : null
  })
  // The state row this build cannot read: the GET answers 409 and says so.
  const unreadable = $derived(query.isError && /unreadable/.test(errorMessage(query.error)))
  const banned = $derived((s?.images ?? []).filter((i) => i.status === 'bad'))
  let pinChoice = $state('')
  let pinVersion = $state('')
  let showHistory = $state(false)
  $effect(() => {
    if (!pinChoice && s) pinChoice = s.pinned?.digest ?? s.current.digest
  })
  let windowStart = $state('')
  let windowEnd = $state('')
  $effect(() => {
    if (s && !windowStart && !windowEnd) {
      windowStart = s.window.start
      windowEnd = s.window.end
    }
  })

  let busy = $state<string | null>(null)
  let line = $state<{ tone: 'ok' | 'danger'; text: string } | null>(null)
  const act = async (action: string, body: Record<string, unknown> = {}) => {
    busy = action
    line = null
    try {
      const r = await postJson<{ message: string }>('/api/fleet/hermes', { action, ...body })
      line = { tone: 'ok', text: r.message }
      await queryClient.invalidateQueries({ queryKey: ['fleet-hermes'] })
    } catch (e) {
      line = { tone: 'danger', text: errorMessage(e) }
    } finally {
      busy = null
    }
  }
  const rollBack = async () => {
    const to =
      rollout?.open && rollout.kind !== 'rollback' && rollout.kind !== 'catchup'
        ? rollout.fromLabel
        : (s?.previous?.label ?? s?.compiled.label)
    const ok = await confirm({
      title: 'Roll back the agent image?',
      message: `Every agent on the version you are leaving moves to ${to ?? 'the previous version'} within a minute, one at a time — now, even mid-conversation — each offline for its restart. An agent someone stopped stays stopped and starts on ${to ?? 'it'} next time. A version that came from stable is marked bad so it is not rolled forward again (Allow again lifts that); a pinned version is not.`,
      confirmLabel: 'Roll back',
    })
    if (ok) await act('rollback')
  }
  const accept = async (i: ImageView) => {
    const ok = await confirm({
      title: `Roll out ${i.label} anyway?`,
      message: `Talaria checked the Hermes internals it relies on, and this release changed some: ${i.contractProblems.join('; ')}. Rolled out anyway, it still goes through the canary and the soaks, one agent at a time. These problems are not checked again; one the canary finds that is not listed here holds it again.`,
      confirmLabel: 'Accept and roll out',
    })
    if (ok) await act('accept', { digest: i.digest })
  }
  const reset = async () => {
    const ok = await confirm({
      title: 'Reset the agent image state?',
      message: 'Its saved state cannot be read by this version of Talaria (written by a later one, or damaged), so automatic updates are paused. Reset keeps it as it is under a new name and starts fresh: every agent stays on the image it runs, and updates resume.',
      confirmLabel: 'Reset',
    })
    if (ok) {
      await act('reset')
      await query.refetch()
    }
  }
  const rollNow = async () => {
    const ok = await confirm({
      title: 'Roll out now?',
      message: 'The new Hermes is tested in a canary and then every agent restarts onto it, one at a time, now instead of in the maintenance window. An agent in a conversation waits until it is free. An agent whose Hermes does not say whether it is working is restarted anyway, and so is one that failed an earlier catch-up.',
      confirmLabel: 'Roll now',
    })
    if (ok) await act('roll')
  }
  const pinTo = async (body: { digest?: string; version?: string }, label: string) => {
    const ok = await confirm({
      title: `Pin every agent to ${label}?`,
      message: `Every agent restarts onto ${label} now, one at a time, and stays there until you unpin. A version that never ran here is tested in a canary first. An agent working waits until it is free; one whose Hermes does not say whether it is working is restarted anyway.`,
      confirmLabel: 'Pin',
    })
    if (ok) await act('pin', body)
  }
  const allowAgain = async (i: ImageView) => {
    const ok = await confirm({
      title: `Allow ${i.label} again?`,
      message: `It was marked bad: ${i.reason ?? 'no reason recorded'}. Once allowed, it can be pinned, and stable can roll it out again in a maintenance window, through the canary.`,
      confirmLabel: 'Allow again',
    })
    if (ok) await act('allow', { digest: i.digest })
  }

  const TONE = { ok: 'text-fg', danger: 'text-danger' } as const
  const agentNote = (a: AgentView) => {
    if (!rollout?.open) return null
    if (rollout.agent === a.slug) return 'moving now'
    if (rollout.deferred.includes(a.slug)) return 'busy all window: moves in a later one'
    const skip = rollout.skipped.find((k) => k.slug === a.slug)
    if (skip) return `waiting: ${skip.why}`
    if (rollout.phase === 'rolling' && !rollout.moved.includes(a.slug)) return 'queued'
    return null
  }
</script>

<Panel class="px-4 py-3">
  <div class="mb-1 flex min-h-6 items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent image</span>
    {#if rollout?.open}<StatusDot status="accent" pulse title="A rollout is in progress" />{/if}
  </div>
  {#if query.isPending}
    <p class="py-2 font-sans text-xs text-muted">Reading the agent image…</p>
  {:else if query.isError || !s}
    <p class="py-2 font-sans text-xs text-danger">Could not read the agent image: {errorMessage(query.error)}</p>
    {#if unreadable}
      <p class="pb-2 font-sans text-xs text-muted">
        Agents keep running, and hiring, starting and editing them work; only automatic image updates are paused.
      </p>
      <Button variant="outline" size="sm" disabled={busy !== null} onclick={reset}>Reset the state</Button>
      {#if line}<p class="pt-2 font-sans text-xs {TONE[line.tone]}">{line.text}</p>{/if}
    {/if}
  {:else}
    <div class="space-y-3 py-2">
      <div>
        <div class="font-sans text-sm text-fg">
          Hermes {s.current.label}
          <span class="text-muted">·</span>
          {#if s.standing === 'following'}
            Automatic: follows <span class="font-mono text-xs">stable</span>
          {:else if s.standing === 'pinned'}
            Pinned to {s.pinned?.label}
          {:else}
            Set by the operator
          {/if}
        </div>
        {#if s.standing === 'off'}
          <div class="font-sans text-xs text-muted">{s.offReason}</div>
        {:else}
          <div class="font-sans text-xs text-muted">
            {#if s.lastCheck}
              Checked {ago(s.lastCheck.at)}{#if s.lastCheck.label}: stable is {s.lastCheck.label}{/if}.
            {:else}
              Not checked yet.
            {/if}
            {#if until(s.nextCheckAt)}Next check {until(s.nextCheckAt)}.{/if}
            Maintenance window {s.window.start}–{s.window.end}
            <span class={s.window.zoneSet ? '' : 'text-danger'}>{s.window.zoneSaid}</span>{s.window.open ? ' (open now)' : ''}.
            A new release waits {s.minStableHours} h on stable first.
          </div>
          {#if s.lastCheck?.error}
            <div class="font-sans text-xs text-danger">The last check failed: {s.lastCheck.error}</div>
          {/if}
        {/if}
      </div>

      {#if rollout}
        <div class="rounded-md border border-line-subtle px-3 py-2">
          <div class="flex items-center gap-2 font-sans text-sm text-fg">
            {#if rollout.open}<WaitingMark site="manage/hermes-rollout" size={12} />{/if}
            {heading(rollout)} {rollout.label}
            {#if rollout.open}<span class="text-muted">— {PHASE[rollout.phase]}</span>{/if}
          </div>
          {#if rollout.detail && !held}
            <div class="font-sans text-xs {rollout.phase === 'failed' ? 'text-danger' : 'text-muted'}">{rollout.detail}</div>
          {/if}
          <div class="font-sans text-xs text-muted">
            Started {ago(rollout.startedAt)}{#if rollout.moved.length > 0}, {rollout.moved.length} of {s.agents.length} moved{/if}.
          </div>
          {#if held}
            <div class="mt-2 space-y-1 border-t border-line-subtle pt-2">
              <div class="font-sans text-xs text-danger">Held for review — a Hermes internal Talaria relies on changed:</div>
              <ul class="list-disc pl-4">
                {#each held.contractProblems as p, n (n)}
                  <li class="font-sans text-xs text-muted">{p}</li>
                {/each}
              </ul>
              <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => held && accept(held)}>
                Accept and roll out
              </Button>
            </div>
          {/if}
        </div>
      {/if}

      {#if s.agents.length > 0}
        <ul class="divide-y divide-line-subtle">
          {#each s.agents as a (a.slug)}
            <li class="flex items-baseline justify-between gap-3 py-1.5">
              <span class="min-w-0 truncate font-sans text-sm text-fg">{a.displayName}</span>
              <span class="shrink-0 font-sans text-xs text-muted">
                {#if agentNote(a)}<span class="mr-2">{agentNote(a)}</span>{/if}
                <span class="font-mono">{a.image?.label ?? (a.onChannel ? 'operator image' : 'workbench image')}</span>
              </span>
            </li>
          {/each}
        </ul>
      {/if}

      {#if s.standing !== 'off'}
        <div class="flex flex-wrap gap-2 border-t border-line pt-3">
          <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('check')}>
            {#if busy === 'check'}<WaitingMark site="manage/hermes-check" size={12} />{:else}<RefreshCw size={14} />{/if}
            {busy === 'check' ? 'Checking…' : 'Check now'}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null || s.standing === 'pinned' || (!stableNewer && !rollout?.open) || !!rollout?.forced}
            onclick={rollNow}
            title={rollout?.forced ? 'Already rolling out now' : undefined}
          >
            <Rocket size={14} />
            Roll now
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null || (rollout?.open && rollout.kind === 'rollback') || (!rollout?.open && !s.previous && s.current.digest === s.compiled.digest)}
            onclick={rollBack}
          >
            <Undo2 size={14} />
            Roll back
          </Button>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          {#if s.standing === 'pinned'}
            <Button variant="ghost" size="sm" disabled={busy !== null} onclick={() => act('unpin')}>
              <PinOff size={14} />
              Unpin: follow stable
            </Button>
          {:else}
            <Select size="sm" bind:value={pinChoice} aria-label="Version to pin">
              {#each pinnable as i (i.digest)}
                <option value={i.digest}>{i.label}</option>
              {/each}
            </Select>
            <Button
              variant="ghost"
              size="sm"
              disabled={busy !== null || !pinChoice}
              onclick={() => pinTo({ digest: pinChoice }, pinnable.find((i) => i.digest === pinChoice)?.label ?? 'that version')}
            >
              <Pin size={14} />
              Pin
            </Button>
            <Input size="sm" class="w-24 font-[var(--font-mono)]" bind:value={pinVersion} placeholder="v0.22.5" aria-label="Or pin a version tag" />
            <Button
              variant="ghost"
              size="sm"
              disabled={busy !== null || !/^v\d+\.\d+\.\d+$/.test(pinVersion.trim())}
              onclick={() => pinTo({ version: pinVersion.trim() }, pinVersion.trim())}
            >
              Pin version
            </Button>
          {/if}
        </div>
        {#if banned.length > 0}
          <div>
            <div class="font-sans text-xs text-muted">Marked bad here — never rolled out, never pinned, until allowed again:</div>
            <ul class="divide-y divide-line-subtle">
              {#each banned as i (i.digest)}
                <li class="flex items-baseline justify-between gap-3 py-1.5">
                  <span class="min-w-0 font-sans text-xs">
                    <span class="font-mono text-fg">{i.label}</span>
                    <span class="text-muted"> — {i.reason ?? 'no reason recorded'}</span>
                  </span>
                  <Button variant="ghost" size="sm" disabled={busy !== null} onclick={() => allowAgain(i)}>Allow again</Button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-sans text-xs text-muted">Window</span>
          <Input size="sm" class="w-20 font-[var(--font-mono)]" bind:value={windowStart} aria-label="Window start (HH:MM)" />
          <span class="font-sans text-xs text-muted">to</span>
          <Input size="sm" class="w-20 font-[var(--font-mono)]" bind:value={windowEnd} aria-label="Window end (HH:MM)" />
          <Button
            variant="ghost"
            size="sm"
            disabled={busy !== null || (windowStart === s.window.start && windowEnd === s.window.end)}
            onclick={() => act('window', { start: windowStart, end: windowEnd })}
          >
            Save
          </Button>
        </div>
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
