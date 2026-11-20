<script lang="ts">
  import { AtSign, Bell, BellDot, BellOff, BellRing, Clock } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cogMenuIcon } from '@/components/ui/settings-cog'
  import { cn } from '@/lib/cn'
  import {
    BOARD_PREFS,
    NOTIFY_BOARDS_KEY,
    QUICK_PRESETS,
    availableCadences,
    boardNotifyKey,
    clearBoardNotify,
    controlName,
    glyphOf,
    presetLabel,
    putBoardNotify,
    refusalMessage,
    sourceWords,
    summary,
    useBoardNotify,
    writeFrom,
    type BoardNotifyAnswer,
    type BoardNotifyWrite,
  } from '@/lib/board-notify'
  import { useNotifications } from '@/lib/notifications'
  import { featureSet } from '@/lib/notify-center'

  // The board's notification control, beside its Board settings gear: what
  // this person hears about this board's tickets — All activity,
  // Participating, Mentions & assignments, Off — in one click, and the door
  // to the rest (Custom events, the daily digest, the board's own Alerts and
  // Email) on Board settings → Notifications. Every board role has it: a
  // viewer chooses what they hear about a board they can read.
  //
  // A value-faced quick setter, not a settings door (UI-CONVENTIONS
  // "Settings cog"): its glyph IS the level (bell-ring, bell, @, bell-off,
  // bell-dot for Custom; a clock mark for the daily digest), its name says
  // whose setting and what the value means ("Board notifications:
  // participating (your default)"), and its menu ends in the gear's
  // "Notification settings". The server holds the setting; a write puts its
  // answer into the query, so the bell, the tab and Settings never disagree.
  // Hidden on an api that does not serve `board-prefs` (lib/board-notify.ts).
  let { boardId, onSettings }: { boardId: string; onSettings: () => void } = $props()

  const qc = useQueryClient()
  const notifications = useNotifications()
  const served = $derived(featureSet(notifications.data).has(BOARD_PREFS))
  const query = useBoardNotify(
    () => boardId,
    () => served,
  )
  const data = $derived(query.data)
  let saving = $state(false)

  async function write(run: () => Promise<BoardNotifyAnswer>) {
    if (saving) return
    saving = true
    try {
      const r = await run()
      qc.setQueryData(boardNotifyKey(boardId), r)
      void qc.invalidateQueries({ queryKey: NOTIFY_BOARDS_KEY })
    } catch (e) {
      await alert({ title: 'Notifications not changed', message: refusalMessage(e) })
    } finally {
      saving = false
    }
  }
  const put = (w: BoardNotifyWrite) => void write(() => putBoardNotify(boardId, w))

  const items = (): ContextMenuEntry[] => {
    if (!data) return []
    const { effective: e, registry } = data
    const entries: ContextMenuEntry[] = QUICK_PRESETS.map((p) => ({
      label: presetLabel(registry, p),
      checked: e.preset === p,
      // Off says, before it is chosen, what still comes through.
      title: p === 'off' ? summary({ ...e, preset: 'off', events: [] }, registry, false) : undefined,
      onSelect: () => put(writeFrom(e, { preset: p }, registry)),
    }))
    // Custom is chosen event by event, on the tab.
    entries.push({ label: e.preset === 'custom' ? presetLabel(registry, 'custom') : `${presetLabel(registry, 'custom')}…`, checked: e.preset === 'custom', onSelect: onSettings })
    const digest = availableCadences(registry).find((c) => c.id === 'digest')
    if (digest && e.preset !== 'off') {
      entries.push('sep', {
        label: digest.label,
        checked: e.cadence === 'digest',
        onSelect: () => put(writeFrom(e, { cadence: e.cadence === 'digest' ? 'instant' : 'digest' }, registry)),
      })
    }
    if (data.source === 'board') {
      entries.push('sep', {
        label: 'Use my default',
        title: `Follow your default for every board: ${presetLabel(registry, data.default.effective.preset)}`,
        onSelect: () => void write(() => clearBoardNotify(boardId)),
      })
    }
    entries.push('sep', { label: 'Notification settings', icon: cogMenuIcon, onSelect: onSettings })
    return entries
  }

  const title = $derived(controlName(data ?? null, data?.registry))
  const glyph = $derived(data ? glyphOf(data.effective) : 'bell')
  const digest = $derived(data?.effective.cadence === 'digest' && data.effective.preset !== 'off')
</script>

{#if served && data}
  <div class="flex shrink-0 items-center" data-testid="board-notify">
    <DropdownMenu align="right" {items}>
      {#snippet trigger(open)}
        <IconButton
          size="md"
          {title}
          aria-haspopup="menu"
          aria-expanded={open}
          active={open}
          disabled={saving}
          data-preset={data.effective.preset}
          data-source={data.source}
          data-cadence={data.effective.cadence}
          class={cn('focus:text-fg', data.effective.preset === 'off' && 'text-muted')}
        >
          <span class="relative inline-flex">
            {#if glyph === 'bell-ring'}
              <BellRing size={16} />
            {:else if glyph === 'at-sign'}
              <AtSign size={16} />
            {:else if glyph === 'bell-off'}
              <BellOff size={16} />
            {:else if glyph === 'bell-dot'}
              <BellDot size={16} />
            {:else}
              <Bell size={16} />
            {/if}
            {#if digest}
              <Clock size={9} strokeWidth={2.5} class="absolute -bottom-1 -right-1.5 rounded-full bg-[var(--theme-panel)]" aria-hidden="true" />
            {/if}
          </span>
        </IconButton>
      {/snippet}
      {#snippet footer()}
        <p class="font-sans text-[11px] leading-4 text-muted" data-board-notify-source>
          {sourceWords(data.source)}.
        </p>
      {/snippet}
    </DropdownMenu>
  </div>
{:else if served && query.isPending}
  <!-- Holds the bell's place while it loads, so the gear beside it does not
       jump. A failed read draws nothing: no value is claimed. -->
  <Skeleton class="h-8 w-8 shrink-0 rounded-md" />
{/if}
