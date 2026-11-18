<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import { focusRing } from '@/components/ui/control'
  import Input from '@/components/ui/Input.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { useNotifyPatch } from '@/lib/notify-center'
  import { DEFAULT_SCHEDULE, dndView, safeZone, toggleDay, validSchedule, WEEKDAYS } from '@/lib/notify-dnd'
  import type { DndSchedule, DndWire } from '@/lib/notify-wire'
  import { cn } from '@/lib/cn'
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

<!-- Rows for the Do not disturb card. -->
<div data-notify-schedule class="[&>*+*]:border-t [&>*+*]:border-line-subtle">
  <SettingsRow label="Working hours">
    <Toggle checked={sched.enabled} onChange={(on) => void save({ ...sched, enabled: on })} title="Working hours" />
  </SettingsRow>
  {#if sched.enabled}
    <SettingsRow label="Days">
      <!-- The frame and thumb of a Segmented, with any number of days on. -->
      <div class="inline-flex gap-0.5 rounded-md border border-line p-0.5" data-schedule-days>
        {#each WEEKDAYS as d (d.n)}
          {@const on = sched.days.includes(d.n)}
          <button
            type="button"
            aria-pressed={on}
            onclick={() => void save({ ...sched, days: toggleDay(sched.days, d.n) })}
            class={cn(
              'min-w-10 select-none rounded border px-2 py-[3px] font-sans text-[13px] font-medium leading-5 transition-colors',
              focusRing,
              on ? 'border-line-strong bg-raised text-fg shadow-[var(--theme-shadow-1)]' : 'border-transparent text-muted hover:text-fg',
            )}
          >
            {d.short}
          </button>
        {/each}
      </div>
    </SettingsRow>
    <SettingsRow label="Hours">
      {#snippet hint()}
        {zone} time{sched.start > sched.end ? ', ending the next day' : ''}.
        <span data-schedule-now>
          {#if draft}
            <!-- an edit in flight: the server has not evaluated it yet -->
          {:else if now.active && now.reason === 'schedule'}
            Right now: {now.label?.toLowerCase()}.
          {:else if now.active && now.reason === 'paused'}
            Right now: paused, which wins over working hours.
          {:else}
            Right now: inside your working hours.
          {/if}
        </span>
        {#if invalid}<span class="block text-danger" role="alert">{invalid}</span>{/if}
        {#if zone === 'UTC' && browser && browser !== 'UTC'}
          <span class="block text-warning">
            These hours are in UTC because your profile has no time zone.
            <a class="underline" href={p('/settings')}>Set it in Profile</a> to use {browser}.
          </span>
        {/if}
      {/snippet}
      <Input
        type="time"
        size="sm"
        class="w-[132px]"
        aria-label="Working hours start"
        value={sched.start}
        onchange={(e) => void save({ ...sched, start: e.currentTarget.value })}
        data-schedule-start
      />
      <span class="font-sans text-[13px] text-muted">to</span>
      <Input
        type="time"
        size="sm"
        class="w-[132px]"
        aria-label="Working hours end"
        value={sched.end}
        onchange={(e) => void save({ ...sched, end: e.currentTarget.value })}
        data-schedule-end
      />
    </SettingsRow>
  {/if}
  <SettingsRow label="Let Needs you through" hint="Approvals and escalations still alert while paused or outside your hours.">
    <Toggle checked={allowNeedsYou} onChange={(on) => void setAllow(on)} title="Let Needs you through" />
  </SettingsRow>
</div>
