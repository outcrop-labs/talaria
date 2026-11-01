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
  const grid = 'grid grid-cols-[minmax(0,1fr)_repeat(4,3.5rem)_2rem] items-center gap-x-1'
</script>

<div data-notify-matrix>
  <div class={cn(grid, 'border-b border-line pb-2')}>
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Notify me about</span>
    {#each MATRIX_COLUMNS as col (col.id)}
      <span
        class={cn(
          'text-center font-mono text-[10px] uppercase tracking-[0.05em]',
          col.id === 'email' && !emailEnabled ? 'text-ink-dim' : 'text-muted',
        )}
        title={col.id === 'email' && emailNote ? emailNote : col.title}
      >
        {col.label}
      </span>
    {/each}
    <span></span>
  </div>
  {#if emailNote}
    <p class="mt-2 font-sans text-xs text-warning" data-matrix-email-off>{emailNote}</p>
  {/if}
  {#each groups as g (g.category)}
    <div class="mt-4" data-matrix-group={g.category}>
      <div class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">{g.label}</div>
      <div class="font-sans text-xs text-ink-dim">{g.blurb}</div>
      <ul class="mt-1 divide-y divide-line-subtle">
        {#each g.rows as r (r.cls.id)}
          <li class={cn(grid, 'py-2.5')} data-matrix-row={r.cls.id}>
            <div class="min-w-0">
              <div class="text-sm text-fg">{r.cls.label}</div>
              <div class="font-sans text-xs text-muted">{r.cls.blurb}</div>
            </div>
            <span class="flex justify-center" data-matrix-cell="inApp">
              <Checkbox bare checked disabled title="In-app is always on: every notification is kept in your inbox" onChange={() => {}} />
            </span>
            {#each CELLS as ch (ch)}
              <span class="flex justify-center" data-matrix-cell={ch}>
                <Checkbox
                  bare
                  checked={r.channels[ch]}
                  disabled={ch === 'email' && !emailEnabled}
                  title="{MATRIX_COLUMNS.find((c) => c.id === ch)?.label}: {r.cls.label}"
                  onChange={(on) => toggle(r.cls.id, ch, on)}
                />
              </span>
            {/each}
            <span class="flex justify-center">
              {#if !r.atDefault}
                <IconButton size="sm" title="Reset {r.cls.label} to default" onclick={() => void save(resetToDefault(r.cls))}>
                  <RotateCcw size={13} strokeWidth={1.5} />
                </IconButton>
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    </div>
  {/each}
</div>
