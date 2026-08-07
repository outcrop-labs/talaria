<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import CapabilityTags from './CapabilityTags.svelte'
  import { BAND_META, BAND_TEXT, bandOf, reasonOf, rowSummary, type FitnessBand, type FitnessIndexEntry, type ModelRow, type SlotView } from './fitness'

  // THE MATRIX. Rows = registered models, columns = the roles and platform
  // agents an admin can assign one to. This is the "can I swap this model in"
  // answer at a glance, and it is the feature.
  //
  // AN UNTESTED CELL LOOKS UNTESTED. Grey dot, grey tooltip, and it is the
  // DEFAULT for every model with no archived run — the most dangerous thing
  // this page could do is imply a model was checked when it was not.
  let {
    slots,
    models,
    index,
    selected,
    onSelect,
  }: {
    slots: SlotView[]
    models: ModelRow[]
    index: Record<string, FitnessIndexEntry>
    selected: string | null
    onSelect: (model: string) => void
  } = $props()

  const roles = $derived(slots.filter((s) => s.kind === 'role'))
  const agents = $derived(slots.filter((s) => s.kind === 'agent'))
  const ordered = $derived([...roles, ...agents])

  // Tested models first: a page whose top rows are all grey buries the work an
  // admin has already paid for under an alphabetical list of everything else.
  const rows = $derived(
    [...models].sort((a, b) => {
      const at = index[a.id]?.at ?? ''
      const bt = index[b.id]?.at ?? ''
      if (at !== bt) return bt.localeCompare(at)
      return a.id.localeCompare(b.id)
    }),
  )

  const cellTitle = (entry: FitnessIndexEntry | undefined, slot: SlotView): string => {
    const band = bandOf(entry, slot.key)
    const reason = reasonOf(entry, slot.key)
    return `${slot.label} — ${BAND_META[band].label}\n${reason ?? BAND_META[band].blurb}`
  }
</script>

<!-- Twenty-one columns never fit a 4xl page: the table scrolls inside its own
     box (UI-CONVENTIONS: wide content scrolls itself, the page body never
     scrolls sideways) and the model column is sticky so a row stays readable. -->
<div class="overflow-x-auto rounded-lg border border-line">
  <table class="w-full border-collapse text-left">
    <thead>
      <tr class="border-b border-line">
        <th class="sticky left-0 z-10 bg-panel px-3 py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model</th>
        {#each ordered as slot, i (slot.key)}
          <th
            class={cn(
              'px-1 py-2 align-bottom font-mono text-[10px] font-normal tracking-[0.04em] text-ink-dim',
              // The one visible seam between the two registries: roles are the
              // activity classes, platform agents are Talaria's own workers.
              slot.kind === 'agent' && i > 0 && ordered[i - 1]?.kind === 'role' && 'border-l border-line',
            )}
            title="{slot.label} — {slot.hint}{slot.requires.length ? `\nNeeds: ${slot.requires.join(', ')}` : ''}"
          >
            <!-- Vertical column heads: 21 horizontal labels would set the
                 table's width from its header text rather than its data. -->
            <span class="block h-28 whitespace-nowrap [writing-mode:vertical-rl] [text-orientation:mixed]">
              {slot.label}{slot.live ? '' : ' (reserved)'}
            </span>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as m (m.id)}
        {@const entry = index[m.id]}
        {@const summary = rowSummary(entry, ordered)}
        <tr
          class={cn('border-b border-line-subtle transition-colors last:border-0 hover:bg-hover', selected === m.id && 'bg-card')}
        >
          <th scope="row" class={cn('sticky left-0 z-10 bg-panel px-3 py-1.5 font-normal', selected === m.id && 'bg-card')}>
            <button
              type="button"
              onclick={() => onSelect(m.id)}
              class={cn('flex max-w-[22rem] flex-col items-start gap-0.5 rounded text-left', focusGold)}
            >
              <span class="truncate font-mono text-xs text-fg">{m.id}</span>
              <span class="flex flex-wrap items-center gap-1">
                <span class="font-mono text-[10px] {BAND_TEXT[summary.band]}">
                  {summary.counts.ready} ready · {summary.counts.workable} workable · {summary.counts.unfit} unfit · {summary.counts.untested + summary.counts.unbound} untested
                </span>
                <!-- Tier 3 is a fact about the MODEL, not about any one slot,
                     so it sits on the row and never colors a cell. It was
                     computed and shipped in the payload before it was shown
                     anywhere, which made an adversarial run something an admin
                     paid for and could only see by opening the model. -->
                {#if entry?.safety}
                  <span
                    class="font-mono text-[10px] {BAND_TEXT[entry.safety.band]}"
                    title="Adversarial (tier 3): {BAND_META[entry.safety.band].label}. Resistance is the share of safety provocations this model did not take the bait on — scored with the production guard rules."
                  >
                    · safety {BAND_META[entry.safety.band].label.toLowerCase()}{entry.safety.resistance === null
                      ? ''
                      : ` ${Math.round(entry.safety.resistance * 100)}%`}
                  </span>
                {/if}
              </span>
              <CapabilityTags row={m} negativeOnly />
            </button>
          </th>
          {#each ordered as slot, i (slot.key)}
            {@const band = bandOf(entry, slot.key)}
            <td
              class={cn(
                'px-1 py-1.5 text-center',
                slot.kind === 'agent' && i > 0 && ordered[i - 1]?.kind === 'role' && 'border-l border-line',
              )}
            >
              <button
                type="button"
                onclick={() => onSelect(m.id)}
                title={cellTitle(entry, slot)}
                aria-label="{m.id} — {slot.label}: {BAND_META[band].label}"
                class={cn('h-5 w-5 rounded text-[13px] leading-5 transition-colors hover:bg-raised', BAND_TEXT[band], focusGold)}
              >
                {BAND_META[band].glyph}
              </button>
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<!-- The legend is not decoration: `untested` and `no harness` are the two
     bands an admin is most likely to misread as a pass, and they are the two
     that mean the opposite. -->
<div class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1">
  {#each Object.entries(BAND_META) as [band, meta] (band)}
    <span class="flex items-center gap-1.5 font-mono text-[10px] text-muted" title={meta.blurb}>
      <span class={BAND_TEXT[band as FitnessBand]}>{meta.glyph}</span>
      {meta.label}
    </span>
  {/each}
  <Chip class="ml-auto" title="Reserved slots take effect when their surface lands. A verdict is still produced — telling you now that your pick cannot see beats telling you the week it ships.">
    reserved = not wired yet
  </Chip>
</div>
