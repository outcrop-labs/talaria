<script lang="ts">
  import { RotateCcw } from '@lucide/svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'
  import { useNotifyPatch } from '@/lib/notify-center'
  import {
    emailColumnNote,
    matrixGroups,
    MATRIX_COLUMNS,
    mergePatch,
    resetToDefault,
    setChannel,
    type MatrixChannel,
  } from '@/lib/notify-matrix'
  import type { ChannelsPatch, ChannelsPrefs, ClassInfo } from '@/lib/notify-wire'

  // THE MATRIX: what reaches you where, per kind of notification. Rows are the
  // server's registry (`classes[]`, I-12) grouped by category; columns are
  // In-app (always on — the row is the record), Desktop, Push and Email. A
  // click saves at once (`PATCH {channels}`), the cell moves under the cursor,
  // and a refusal rolls it back with the reason. The server rewrites each
  // class's legacy route from it (I-13), so an older app reading `prefs` still
  // behaves as close as it can.
  let {
    classes,
    channels,
    emailEnabled,
    onSaved,
    onError,
  }: {
    classes: readonly ClassInfo[]
    channels: ChannelsPrefs | undefined
    /** The instance email switch (`delivery.emailEnabled`). */
    emailEnabled: boolean
    onSaved?: () => void
    onError?: (message: string) => void
  } = $props()

  const patch = useNotifyPatch()
  let pending = $state<ChannelsPatch>({})
  const groups = $derived(matrixGroups(classes, channels, pending))
  const emailNote = $derived(emailColumnNote(emailEnabled))

  const save = async (change: ChannelsPatch) => {
    pending = mergePatch(pending, change)
    const r = await patch({ channels: change })
    // Drop exactly the keys this save sent, so a second click still in flight
    // keeps its overlay.
    const next: ChannelsPatch = { ...pending }
    for (const [id, v] of Object.entries(change)) {
      const left = { ...next[id] }
      for (const k of Object.keys(v)) delete left[k as keyof typeof left]
      if (Object.keys(left).length) next[id] = left
      else delete next[id]
    }
    pending = next
    if ('error' in r) onError?.(r.error)
    else onSaved?.()
  }

  const CELLS: MatrixChannel[] = ['desktop', 'push', 'email']
  const toggle = (id: string, ch: MatrixChannel, on: boolean) => void save(setChannel(id, ch, on))
  // Wide: the label, then the four channels and the reset in one row. Narrow
  // (under `sm`): the label on its own line across the row, the channels
  // under it in four equal columns, so a label is never squeezed to a word a
  // line.
  const cell = 'flex flex-col items-center gap-1 sm:flex-row sm:justify-center'
  const cellLabel = 'font-sans text-xs leading-4 text-muted sm:hidden'
  const grid =
    'grid grid-cols-[repeat(4,minmax(0,1fr))_2rem] items-center gap-x-1 gap-y-2 px-5 sm:grid-cols-[minmax(0,1fr)_repeat(4,4.5rem)_2rem] sm:gap-y-0'
  const divided = '[&>*+*]:border-t [&>*+*]:border-line-subtle'
</script>

<!-- Row-shaped content for a SettingsGroup: the column heads, then each
     category as a band and its kinds as rows. -->
<div data-notify-matrix class={divided}>
  <!-- The column heads; under `sm` each cell carries its own label instead. -->
  <div class={cn(grid, 'hidden py-2.5 sm:grid')}>
    <span class="font-sans text-xs font-medium leading-4 text-muted">Notify me about</span>
    {#each MATRIX_COLUMNS as col (col.id)}
      <span
        class={cn('text-center font-sans text-xs font-medium leading-4 text-muted', col.id === 'email' && !emailEnabled && 'opacity-60')}
        title={col.id === 'email' && emailNote ? emailNote : col.title}
      >
        {col.label}
      </span>
    {/each}
    <span></span>
  </div>
  {#if emailNote}
    <p class="px-5 py-3 font-sans text-[13px] leading-5 text-warning" data-matrix-email-off>{emailNote}</p>
  {/if}
  {#each groups as g (g.category)}
    <div class={divided} data-matrix-group={g.category}>
      <div class="bg-raised/40 px-5 py-2 font-sans text-[13px] leading-5">
        <span class="font-medium text-fg">{g.label}</span>
        <span class="ml-2 text-muted">{g.blurb}</span>
      </div>
      <ul class={divided}>
        {#each g.rows as r (r.cls.id)}
          <li class={cn(grid, 'min-h-[60px] py-3')} data-matrix-row={r.cls.id}>
            <div class="col-span-full min-w-0 sm:col-span-1">
              <div class="font-sans text-sm font-medium leading-5 text-fg">{r.cls.label}</div>
              <div class="font-sans text-[13px] leading-5 text-muted">{r.cls.blurb}</div>
            </div>
            <span class={cell} data-matrix-cell="inApp">
              <Checkbox bare checked disabled title="In-app is always on: every notification is kept in your inbox" onChange={() => {}} />
              <span class={cellLabel}>In-app</span>
            </span>
            {#each CELLS as ch (ch)}
              <span class={cell} data-matrix-cell={ch}>
                <Checkbox
                  bare
                  checked={r.channels[ch]}
                  disabled={ch === 'email' && !emailEnabled}
                  title="{MATRIX_COLUMNS.find((c) => c.id === ch)?.label}: {r.cls.label}"
                  onChange={(on) => toggle(r.cls.id, ch, on)}
                />
                <span class={cellLabel}>{MATRIX_COLUMNS.find((c) => c.id === ch)?.label}</span>
              </span>
            {/each}
            <span class="flex justify-center">
              {#if !r.atDefault}
                <IconButton size="sm" title="Reset {r.cls.label} to default" onclick={() => void save(resetToDefault(r.cls))}>
                  <RotateCcw size={14} strokeWidth={1.5} />
                </IconButton>
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    </div>
  {/each}
</div>
