<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { useNotifyPatch } from '@/lib/notify-center'
  import { DEFAULT_SCHEDULE, dndView, safeZone, scheduleSummary, toggleDay, validSchedule, WEEKDAYS } from '@/lib/notify-dnd'
  import type { DndSchedule, DndWire } from '@/lib/notify-wire'
  import { detectedZone } from '@/lib/timezone'
  import { p } from '@/router'

  // WORKING HOURS (I-14): inside them notifications alert as usual; outside
  // them they land quietly — in the inbox and on the bell, without a banner,
  // a push or a sound — except Needs you, unless the person says otherwise.
  // A schedule whose start is after its end crosses midnight. The server
  // evaluates it in the person's zone (`dnd_state`); this editor writes it
  // (`PATCH {dnd: {schedule}}`) and shows the server's answer for right now.
  let {
    dnd,
    onSaved,
    onError,
  }: {
    dnd: DndWire | undefined
    onSaved?: () => void
    onError?: (message: string) => void
  } = $props()

  const patch = useNotifyPatch()
  // The draft follows the server until the person edits it; a save hands it
  // back to the server's answer.
  let draft = $state<DndSchedule | null>(null)
  const stored = $derived(dnd?.schedule ?? null)
  const sched: DndSchedule = $derived(draft ?? stored ?? { ...DEFAULT_SCHEDULE, enabled: false })
  const invalid = $derived(validSchedule(sched))
  const zone = $derived(safeZone(dnd?.timeZone))
  const browser = detectedZone()
  const now = $derived(dndView(dnd, Date.now(), zone))

  // Each save sends the whole schedule; only the latest one hands the draft
  // back, so an earlier answer landing late never undoes a later edit.
  let latest = 0
  const save = async (next: DndSchedule) => {
    draft = next
    if (validSchedule(next)) return
    const mine = ++latest
    const r = await patch({ dnd: { schedule: next } })
    if (mine !== latest) return
    if ('error' in r) onError?.(r.error)
    else {
      draft = null
      onSaved?.()
    }
  }

  const allowNeedsYou = $derived(dnd?.allowNeedsYou ?? true)
  const setAllow = async (on: boolean) => {
    const r = await patch({ dnd: { allowNeedsYou: on } })
    if ('error' in r) onError?.(r.error)
    else onSaved?.()
  }
</script>

<div data-notify-schedule>
  <Toggle
    checked={sched.enabled}
    onChange={(on) => void save({ ...sched, enabled: on })}
    label="Working hours"
  />
  <p class="mt-1 max-w-xl font-sans text-xs text-muted">
    Outside these hours notifications arrive quietly: in your inbox and on the bell, with no banner, push or email until
    your hours start again.
  </p>
  {#if sched.enabled}
    <div class="mt-3 flex flex-wrap items-center gap-1.5" data-schedule-days>
      {#each WEEKDAYS as d (d.n)}
        <Chip onSelect={() => void save({ ...sched, days: toggleDay(sched.days, d.n) })} selected={sched.days.includes(d.n)}>
          {d.short}
        </Chip>
      {/each}
    </div>
    <div class="mt-3 flex flex-wrap items-center gap-2">
      <label class="flex items-center gap-2 font-sans text-xs text-muted">
        From
        <Input
          type="time"
          size="sm"
          class="w-32"
          value={sched.start}
          onchange={(e) => void save({ ...sched, start: e.currentTarget.value })}
          data-schedule-start
        />
      </label>
      <label class="flex items-center gap-2 font-sans text-xs text-muted">
        to
        <Input
          type="time"
          size="sm"
          class="w-32"
          value={sched.end}
          onchange={(e) => void save({ ...sched, end: e.currentTarget.value })}
          data-schedule-end
        />
      </label>
      <span class="font-sans text-xs text-ink-dim">{scheduleSummary(sched)}, {zone}</span>
    </div>
    {#if invalid}<p class="mt-2 font-sans text-xs text-danger" role="alert">{invalid}</p>{/if}
    {#if zone === 'UTC' && browser && browser !== 'UTC'}
      <p class="mt-2 font-sans text-xs text-warning">
        These hours are in UTC because your profile has no time zone.
        <a class="underline" href={p('/settings')}>Set it in Profile</a> to use {browser}.
      </p>
    {/if}
    <p class="mt-2 font-sans text-xs text-muted" data-schedule-now>
      {#if draft}
        <!-- an edit in flight: the server has not evaluated it yet -->
      {:else if now.active && now.reason === 'schedule'}
        Right now: {now.label?.toLowerCase()}.
      {:else if now.active && now.reason === 'paused'}
        Right now: paused, which wins over working hours.
      {:else}
        Right now: inside your working hours.
      {/if}
    </p>
  {/if}
  <div class="mt-4">
    <Toggle checked={allowNeedsYou} onChange={(on) => void setAllow(on)} label="Let Needs you through" />
    <p class="mt-1 max-w-xl font-sans text-xs text-muted">
      Approvals, escalations and anything else blocked on you still alert while you are paused or outside your hours.
    </p>
  </div>
</div>
