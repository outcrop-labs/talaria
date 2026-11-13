<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { RefreshCw, Rocket, Undo2, Pin, PinOff } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
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
      message: `Every agent on the version you are leaving moves to ${to ?? 'the previous version'} within a minute, one at a time, even mid-conversation, and each is offline for its restart. An agent someone stopped stays stopped and starts on ${to ?? 'it'} next time. A version that came from stable is marked bad so it is not rolled forward again (Allow again lifts that); a pinned version is not.`,
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

<SettingsSection title="Agent image">
  {#snippet action()}
    {#if rollout?.open}
      <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="accent" pulse title="A rollout is in progress" />Rolling out</span>
    {/if}
  {/snippet}
  <SettingsGroup>
    {#if query.isPending}
      <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Reading the agent image</p>
    {:else if query.isError || !s}
      <div class="space-y-2 px-5 py-4 font-sans text-[13px] leading-5">
        <p class="text-danger">Could not read the agent image: {errorMessage(query.error)}</p>
        {#if unreadable}
          <p class="text-muted">Agents keep running, and you can still hire, start and edit them. Only automatic image updates are paused.</p>
          <Button variant="outline" size="sm" disabled={busy !== null} onclick={reset}>Reset the state</Button>
          {#if line}<p class={TONE[line.tone]}>{line.text}</p>{/if}
        {/if}
      </div>
    {:else}
      <SettingsRow label={`Hermes ${s.current.label}`}>
        {#snippet meta()}
          <Chip>{s.standing === 'following' ? 'Automatic' : s.standing === 'pinned' ? `Pinned to ${s.pinned?.label}` : 'Set by the operator'}</Chip>
        {/snippet}
        {#snippet hint()}
          {#if s.standing === 'off'}
            {s.offReason}
          {:else}
            {#if s.lastCheck}
              Checked {ago(s.lastCheck.at)}{#if s.lastCheck.label}, stable is {s.lastCheck.label}{/if}.
            {:else}
              Not checked yet.
            {/if}
            {#if until(s.nextCheckAt)}Next check {until(s.nextCheckAt)}.{/if}
            {#if s.lastCheck?.error}<span class="block text-danger">The last check failed: {s.lastCheck.error}</span>{/if}
          {/if}
          {#if line}<span transition:slide={{ duration: 150 }} class="block {TONE[line.tone]}">{line.text}</span>{/if}
        {/snippet}
        {#if s.standing !== 'off'}
          <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('check')}>
            {#if busy === 'check'}<WaitingMark site="manage/hermes-check" size={12} />{:else}<RefreshCw size={14} />{/if}
            {busy === 'check' ? 'Checking' : 'Check now'}
          </Button>
          <SettingsRowMenu
            label="Actions for the agent image"
            items={[
              {
                label: 'Update now',
                icon: [Rocket, { size: 14 }],
                disabled: busy !== null || s.standing === 'pinned' || (!stableNewer && !rollout?.open) || !!rollout?.forced,
                onSelect: rollNow,
              },
              {
                label: 'Roll back',
                icon: [Undo2, { size: 14 }],
                disabled:
                  busy !== null ||
                  (rollout?.open && rollout.kind === 'rollback') ||
                  (!rollout?.open && !s.previous && s.current.digest === s.compiled.digest),
                onSelect: rollBack,
              },
            ]}
          />
        {/if}
      </SettingsRow>

      {#if rollout}
        <div>
          <SettingsRow label={`${heading(rollout)} ${rollout.label}`}>
            {#snippet meta()}
              {#if rollout.open}
                <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><WaitingMark site="manage/hermes-rollout" size={12} />{PHASE[rollout.phase]}</span>
              {/if}
            {/snippet}
            {#snippet hint()}
              Started {ago(rollout.startedAt)}{#if rollout.moved.length > 0}, {rollout.moved.length} of {s.agents.length} moved{/if}.
              {#if rollout.detail && !held}
                <span class={rollout.phase === 'failed' ? 'text-danger' : ''}>{rollout.detail}</span>
              {/if}
            {/snippet}
            {#if held}
              <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => held && accept(held)}>
                Accept and roll out
              </Button>
            {/if}
          </SettingsRow>
          {#if held}
            <div class="px-5 pb-4 font-sans text-[13px] leading-5">
              <p class="text-danger">Held for review. A Hermes internal that Talaria relies on changed:</p>
              <ul class="mt-1 list-disc pl-4 text-muted">
                {#each held.contractProblems as p, n (n)}<li>{p}</li>{/each}
              </ul>
            </div>
          {/if}
        </div>
      {/if}

      {#if s.standing !== 'off'}
        <SettingsRow label="Update window" info={`A new release waits ${s.minStableHours} h on stable before it rolls out here.`}>
          {#snippet hint()}
            Updates roll out between these hours,
            <span class={s.window.zoneSet ? '' : 'text-danger'}>{s.window.zoneSaid}</span>{s.window.open ? ' (open now)' : ''}.
          {/snippet}
          <Input size="sm" class="w-20 tabular-nums" bind:value={windowStart} aria-label="Window start (HH:MM)" />
          <span class="font-sans text-[13px] text-muted">to</span>
          <Input size="sm" class="w-20 tabular-nums" bind:value={windowEnd} aria-label="Window end (HH:MM)" />
          <Button
            variant="outline"
            size="md"
            disabled={busy !== null || (windowStart === s.window.start && windowEnd === s.window.end)}
            onclick={() => act('window', { start: windowStart, end: windowEnd })}
          >
            Save
          </Button>
        </SettingsRow>

        {#if s.standing === 'pinned'}
          <SettingsRow label="Pinned version" hint={`Agents stay on ${s.pinned?.label ?? 'this version'} until you unpin.`}>
            <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => act('unpin')}>
              <PinOff size={14} />
              Unpin
            </Button>
          </SettingsRow>
        {:else}
          <SettingsRow label="Pin a version" hint="Hold every agent on one version until you unpin.">
            <Select size="sm" class="w-28" bind:value={pinChoice} aria-label="Version to pin">
              {#each pinnable as i (i.digest)}
                <option value={i.digest}>{i.label}</option>
              {/each}
            </Select>
            <Button
              variant="outline"
              size="md"
              disabled={busy !== null || !pinChoice}
              onclick={() => pinTo({ digest: pinChoice }, pinnable.find((i) => i.digest === pinChoice)?.label ?? 'that version')}
            >
              <Pin size={14} />
              Pin
            </Button>
          </SettingsRow>
          <SettingsRow label="Pin by tag" hint="For a version this instance has not seen yet.">
            <Input size="sm" class="w-28 font-mono" bind:value={pinVersion} placeholder="v0.22.5" aria-label="Version tag to pin" />
            <Button
              variant="outline"
              size="md"
              disabled={busy !== null || !/^v\d+\.\d+\.\d+$/.test(pinVersion.trim())}
              onclick={() => pinTo({ version: pinVersion.trim() }, pinVersion.trim())}
            >
              <Pin size={14} />
              Pin
            </Button>
          </SettingsRow>
        {/if}

        {#each banned as i (i.digest)}
          <SettingsRow label="Marked bad" hint={i.reason ?? 'No reason recorded.'}>
            {#snippet meta()}<code class="font-mono text-xs text-muted">{i.label}</code>{/snippet}
            <Button variant="outline" size="sm" disabled={busy !== null} onclick={() => allowAgain(i)}>Allow again</Button>
          </SettingsRow>
        {/each}
      {/if}

      {#if s.agents.length > 0}
        <div class="px-5 py-4">
          <div class="mb-1 font-sans text-xs font-medium leading-4 text-muted">Agents on this image</div>
          <ul>
            {#each s.agents as a (a.slug)}
              <li class="flex h-8 items-center justify-between gap-6 font-sans text-[13px] leading-5">
                <span class="min-w-0 truncate text-fg">{a.displayName}</span>
                <span class="shrink-0 text-muted">
                  {#if agentNote(a)}<span class="mr-2">{agentNote(a)}</span>{/if}
                  <span class="font-mono text-xs">{a.image?.label ?? (a.onChannel ? 'operator image' : 'workbench image')}</span>
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
