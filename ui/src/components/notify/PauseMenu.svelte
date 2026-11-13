<script lang="ts">
  import { BellOff, BellRing, CalendarClock } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
  import { useNotifyPatch, useNotifyZone } from '@/lib/notify-center'
  import { useUiVoice } from '@/lib/ui-voice'
  import {
    customUntil,
    dndView,
    localInputValue,
    PAUSE_PRESETS,
    presetUntil,
    TOMORROW_HOUR,
    type PausePreset,
  } from '@/lib/notify-dnd'
  import type { DndWire } from '@/lib/notify-wire'
  import { detectedZone } from '@/lib/timezone'

  // PAUSE NOTIFICATIONS — the account menu's and the bell's, one component.
  // Inline rows, not a nested popover: both hosts are popovers already, and a
  // second portal would count as a click outside the first and close it.
  //
  // A pause is `PATCH {dnd: {pausedUntil}}`; the instant a preset means is
  // computed here in the person's zone (O-19), and the server's evaluator
  // (dnd_state) decides what it silences. Needs-you breaks through unless the
  // person turned that off (Settings → Notifications).
  let {
    dnd,
    onDone,
  }: {
    dnd: DndWire | undefined
    /** After a save (the host closes itself). */
    onDone?: () => void
  } = $props()

  const patch = useNotifyPatch()
  // The zone "tomorrow 9:00" is in: the profile's, else the browser's.
  const notifyZone = useNotifyZone(() => dnd)
  const zone = $derived(notifyZone.current)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let customOpen = $state(false)
  let customValue = $state('')

  const view = $derived(dndView(dnd, Date.now(), zone))
  const browserZone = detectedZone()
  const tomorrowLabel = $derived(
    zone && browserZone && zone !== browserZone
      ? `Until tomorrow, ${TOMORROW_HOUR}:00 (${zone})`
      : `Until tomorrow, ${TOMORROW_HOUR}:00`,
  )

  const save = async (pausedUntil: string | null) => {
    busy = true
    error = null
    const r = await patch({ dnd: { pausedUntil } })
    busy = false
    if ('error' in r) {
      error = r.error
      return
    }
    customOpen = false
    onDone?.()
  }

  const pick = (id: PausePreset) => void save(presetUntil(id, Date.now(), zone))

  const openCustom = () => {
    customOpen = !customOpen
    if (customOpen && !customValue) customValue = localInputValue(Date.now() + 60 * 60_000, zone)
  }

  const saveCustom = () => {
    const until = customUntil(customValue, Date.now(), zone)
    if (!until) {
      error = 'Pick a time in the future'
      return
    }
    void save(until)
  }

  // The dock's account menu and the bell show this in the house voice; the
  // Notifications settings page shows it in the area's, where a menu's rows
  // carry no icon.
  const sans = useUiVoice() === 'sans'
  const row = sans
    ? 'flex w-full items-center gap-2.5 rounded-md px-3 py-1.5 text-left font-sans text-[13px] leading-5 text-fg transition-colors duration-[120ms] hover:bg-hover disabled:opacity-50'
    : 'flex w-full items-center gap-2.5 rounded-md px-3 py-1.5 text-left text-sm text-fg transition-colors duration-[120ms] dither-fill disabled:opacity-50'
</script>

<div data-pause-menu>
  {#if view.active && view.reason === 'paused'}
    <div class="px-3 pb-1 pt-1.5 font-sans text-xs text-muted" data-pause-state>{view.label}</div>
    <button type="button" class={row} disabled={busy} onclick={() => void save(null)} data-pause-resume>
      {#if !sans}<BellRing size={15} class="shrink-0 text-accent" />{/if}
      <span>Resume notifications</span>
    </button>
  {/if}
  {#each PAUSE_PRESETS as p (p.id)}
    <button type="button" class={row} disabled={busy} onclick={() => pick(p.id)} data-pause-preset={p.id}>
      {#if !sans}<BellOff size={15} class="shrink-0 text-muted" />{/if}
      <span>{p.id === 'tomorrow' ? tomorrowLabel : p.label}</span>
    </button>
  {/each}
  <button type="button" class={cn(row, customOpen && 'bg-raised')} disabled={busy} onclick={openCustom} aria-expanded={customOpen} data-pause-custom>
    {#if !sans}<CalendarClock size={15} class="shrink-0 text-muted" />{/if}
    <span>Until a time</span>
  </button>
  {#if customOpen}
    <div class="flex items-center gap-2 px-3 pb-1.5 pt-1">
      <Input
        type="datetime-local"
        size="sm"
        bind:value={customValue}
        aria-label="Pause until"
        onkeydown={(e) => e.key === 'Enter' && saveCustom()}
      />
      <Button size="sm" disabled={busy} onclick={saveCustom}>Pause</Button>
    </div>
    {#if browserZone && zone !== browserZone}
      <div class={sans ? 'px-3 pb-1 font-sans text-xs text-muted' : 'px-3 pb-1 font-sans text-[11px] text-muted' /* voice-ok: the dock's menus */}>In {zone}</div>
    {/if}
  {/if}
  {#if error}<div class="px-3 pb-1.5 font-sans text-xs text-danger" role="alert">{error}</div>{/if}
</div>
