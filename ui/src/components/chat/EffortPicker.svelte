<script lang="ts">
  import { cn } from '@/lib/cn'
  import Popover from '@/components/ui/Popover.svelte'
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import { chipPrimary, popHeader, popRow, popRowSelected } from '@/components/chat/chat-chrome'

  // The composer's effort chip (Mercury spec §7): the agent-chip anatomy —
  // strong border, mono label, 3×12 meter marking where the pick sits on the
  // model's effort ladder — opening the §7 popover (panel bg, mono section
  // header, hover fill, dashed-gold selected row, a bar meter on every row);
  // the shell (ui/Popover) owns the portal/outside-click/Esc mechanics.
  // Rendered by the chat surfaces ONLY with a non-empty `efforts` list, which
  // the server derives from the model's own catalog metadata: a model that
  // publishes no levels gets no chip and its requests carry no effort.
  // '' (the first row) means the model's own default — no parameter sent, and
  // a bar meter with nothing lit.
  let {
    efforts,
    value,
    onChange,
    disabled = false,
  }: {
    /** The levels this model supports, per its catalog metadata — the picker's
     *  whole option list; nothing else is offered. */
    efforts: string[]
    /** '' = the model's default (no parameter sent). */
    value: string
    onChange: (v: string) => void
    disabled?: boolean
  } = $props()

  let open = $state(false)

  // Where the pick sits on the ladder: nothing lit at the model's default,
  // otherwise every bar up to and including the picked level — the same
  // read the tier chip gives ("how far up the ladder am I?").
  const lit = $derived(Math.max(0, efforts.indexOf(value) + 1))
  const label = $derived(value || 'auto')
</script>

<Popover bind:open follow up offset={6} class="min-w-48 overflow-hidden">
  {#snippet trigger()}
    <button
      type="button"
      disabled={disabled}
      class={cn(chipPrimary, 'shrink-0')}
      title="Reasoning effort for this reply"
      aria-haspopup="listbox"
      aria-expanded={open}
    >
      <span class="max-w-24 truncate">{label}</span>
      <MeterBars total={efforts.length} {lit} />
    </button>
  {/snippet}
  {#snippet content(close)}
    <div class={popHeader}>Reasoning effort</div>
    <button
      type="button"
      onclick={() => {
        onChange('')
        close()
      }}
      class={cn(popRow, value === '' ? popRowSelected : 'text-muted')}
    >
      <span class="min-w-0 flex-1 truncate">auto</span>
      <span class="shrink-0 text-right font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">model default</span>
    </button>
    {#each efforts as level, i (level)}
      <button
        type="button"
        onclick={() => {
          onChange(level)
          close()
        }}
        class={cn(popRow, level === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1 truncate">{level}</span>
        <MeterBars total={efforts.length} lit={i + 1} class="shrink-0" />
      </button>
    {/each}
  {/snippet}
</Popover>
