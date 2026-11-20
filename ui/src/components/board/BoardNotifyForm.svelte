<script lang="ts">
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Radio from '@/components/ui/Radio.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import { cn } from '@/lib/cn'
  import {
    availableCadences,
    availableEvents,
    channelChoice,
    channelValue,
    expandWrite,
    eventLabel,
    refusalMessage,
    scopeWords,
    sourceWords,
    summary,
    writeFrom,
    type BoardCadence,
    type BoardChannels,
    type BoardEffective,
    type BoardNotifyState,
    type BoardNotifyWrite,
    type BoardPreset,
    type BoardRegistry,
    type BoardScope,
    type ChannelChoice,
  } from '@/lib/board-notify'

  // One board-notification setting as a form: the tab in Board settings
  // (this person's setting for that board) and the default for every board in
  // Settings → Notifications draw it alike. Every control saves on change
  // (UI-CONVENTIONS: pickers apply immediately) and shows the server's answer;
  // while a write is on its way it shows what was asked, and a refusal puts
  // the old value back with the server's sentence. The words for presets,
  // events and cadences are the server's registry.
  let {
    value,
    registry,
    channelsServed,
    subject,
    onWrite,
    onReset,
    resetLabel,
    bare = false,
  }: {
    value: BoardNotifyState
    registry: BoardRegistry
    /** The board's own Alerts and Email are served (`board-channels`). */
    channelsServed: boolean
    /** Whose setting: one board's, or the default for every board. */
    subject: 'board' | 'default'
    /** Write the whole setting; rejects with the refusal. */
    onWrite: (w: BoardNotifyWrite) => Promise<unknown>
    /** Back to the layer underneath (this board → the default; the default →
     *  the standard one). Offered only while there is something to undo. */
    onReset?: () => Promise<unknown>
    resetLabel?: string
    /** Rows straight on a surface that is already a panel (a dialog). */
    bare?: boolean
  } = $props()

  const uid = $props.id()
  const autosave = useAutosave()
  let pending = $state<BoardEffective | null>(null)
  let busy = $state(false)
  let error = $state<string | null>(null)
  const eff = $derived(pending ?? value.effective)
  const ownLayer = $derived(subject === 'board' ? value.source === 'board' : value.source === 'default')

  async function change(c: Partial<Omit<BoardNotifyWrite, 'channels'>> & { channels?: Partial<BoardChannels> }, key: string) {
    if (busy) return
    const w = writeFrom(eff, c, registry)
    error = null
    pending = expandWrite(w, registry)
    busy = true
    try {
      await onWrite(w)
      autosave.flash(key)
    } catch (e) {
      error = refusalMessage(e)
    } finally {
      pending = null
      busy = false
    }
  }

  async function reset() {
    if (busy || !onReset) return
    error = null
    busy = true
    try {
      await onReset()
      autosave.flash('preset')
    } catch (e) {
      error = refusalMessage(e)
    } finally {
      busy = false
    }
  }

  function toggleEvent(id: string, on: boolean) {
    const next = on ? [...eff.events, id] : eff.events.filter((e) => e !== id)
    if (next.length === 0) {
      error = 'Pick at least one event, or choose Off.'
      return
    }
    void change({ events: next }, 'preset')
  }

  // A preset's line under its name, from the registry: the events it covers
  // (those that can arrive today) and over which tickets.
  function presetHint(p: BoardRegistry['presets'][number]): string {
    if (p.events === null) return 'Choose the events, and which tickets.'
    if (p.events.length === 0) return `Nothing from ${subject === 'board' ? 'this board' : 'those boards'} — assignments and @mentions included.`
    const offered = new Set(availableEvents(registry).map((e) => e.id))
    const names = p.events.filter((id) => offered.has(id)).map((id) => eventLabel(registry, id))
    return `${names.join(', ')} — on ${scopeWords(p.scope ?? 'mine')}.`
  }

  const cadences = $derived(availableCadences(registry))
  const SCOPES: Array<{ id: BoardScope; label: string }> = [
    { id: 'mine', label: 'Tickets I’m in' },
    { id: 'board', label: 'Every ticket' },
  ]
  const CHANNELS: Array<{ id: ChannelChoice; label: string; title: string }> = [
    { id: 'settings', label: 'From settings', title: 'As Settings → Notifications says for each kind' },
    { id: 'on', label: 'On', title: 'On for this board’s notifications' },
    { id: 'off', label: 'Off', title: 'Off for this board’s notifications' },
  ]
  const whose = $derived(subject === 'board' ? 'this board' : 'every board without a setting of its own')
</script>

<div data-board-notify-form={subject} data-preset={eff.preset} data-source={value.source} class="space-y-3">
  <div class="font-sans text-[13px] leading-5">
    <p class="text-fg" data-board-notify-summary>{summary(eff, registry, channelsServed, subject)}</p>
    <p class="mt-1 flex flex-wrap items-center gap-x-2 text-muted">
      <span data-board-notify-source>{subject === 'board' ? sourceWords(value.source) : value.source === 'default' ? 'Your own default.' : 'The standard default: Participating.'}</span>
      {#if onReset && ownLayer}
        <button
          type="button"
          class="text-accent underline-offset-2 hover:underline disabled:opacity-50"
          disabled={busy}
          onclick={() => void reset()}
          data-board-notify-reset
        >
          {resetLabel ?? 'Reset'}
        </button>
      {/if}
    </p>
  </div>

  <SettingsGroup {bare}>
    <SettingsRow label="Notify me about" hint={`What reaches you from ${whose}. Only yours — nobody else’s notifications change.`} stacked saved={autosave.saved('preset')}>
      <!-- One block: a stacked row lays its children out in a row. -->
      <div class="w-full space-y-3">
      <div role="radiogroup" aria-label="Notify me about" class="space-y-2.5">
        {#each registry.presets as p (p.id)}
          <Radio
            name={`board-notify-${uid}`}
            checked={eff.preset === p.id}
            disabled={busy}
            onChange={() => void change({ preset: p.id as BoardPreset }, 'preset')}
            class="items-start"
          >
            {#snippet label()}
              <span class="flex min-w-0 flex-col" data-preset-option={p.id}>
                <span class="font-sans text-[13px] leading-5 text-fg">{p.label}</span>
                <span class="font-sans text-xs leading-4 text-muted">{presetHint(p)}</span>
              </span>
            {/snippet}
          </Radio>
        {/each}
      </div>
      {#if eff.preset === 'custom'}
        <div class="space-y-3 border-t border-line-subtle pt-3" data-board-notify-custom>
          <div class="flex flex-wrap items-center gap-3">
            <span class="font-sans text-xs text-muted">Which tickets</span>
            <Segmented options={SCOPES} value={eff.scope} onChange={(s) => void change({ scope: s }, 'preset')} />
          </div>
          <div class="grid gap-2 sm:grid-cols-2">
            {#each availableEvents(registry) as ev (ev.id)}
              {@const needsBoard = ev.boardWideOnly && eff.scope === 'mine'}
              <Checkbox
                checked={eff.events.includes(ev.id) && !needsBoard}
                disabled={busy || needsBoard}
                title={needsBoard ? 'Only with “Every ticket” — nobody is in a ticket before it exists' : undefined}
                label={ev.label}
                onChange={(on) => toggleEvent(ev.id, on)}
                class="text-[13px]"
              />
            {/each}
          </div>
        </div>
      {/if}
      </div>
    </SettingsRow>

    {#if cadences.length > 1 && eff.preset !== 'off'}
      <SettingsRow
        label="When"
        hint="As it happens, or rolled up once a day at the daily digest’s hour. Assignments, @mentions and tickets you watch yourself still come at once."
        saved={autosave.saved('cadence')}
      >
        <Segmented
          options={cadences.map((c) => ({ id: c.id, label: c.label }))}
          value={eff.cadence}
          onChange={(c: BoardCadence) => void change({ cadence: c }, 'cadence')}
        />
      </SettingsRow>
    {/if}

    {#if channelsServed}
      <SettingsRow
        label="Alerts"
        hint="The banner and the push. Your inbox keeps every notification either way."
        saved={autosave.saved('alerts')}
      >
        <div data-board-notify-channel="alerts" class={cn(busy && 'pointer-events-none opacity-60')}>
          <Segmented
            options={CHANNELS}
            value={channelChoice(eff.channels.alerts)}
            onChange={(c) => void change({ channels: { alerts: channelValue(c) } }, 'alerts')}
          />
        </div>
      </SettingsRow>
      <SettingsRow label="Email" hint="The missed-activity email." saved={autosave.saved('email')}>
        <div data-board-notify-channel="email" class={cn(busy && 'pointer-events-none opacity-60')}>
          <Segmented
            options={CHANNELS}
            value={channelChoice(eff.channels.email)}
            onChange={(c) => void change({ channels: { email: channelValue(c) } }, 'email')}
          />
        </div>
      </SettingsRow>
    {/if}
  </SettingsGroup>
  {#if error}<p class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>{/if}
</div>
