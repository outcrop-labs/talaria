<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import CapabilityTags from './CapabilityTags.svelte'
  import { SAFETY_META, BAND_META, BAND_TEXT, bandOf, ms, reasonOf, rowSummary, speedTitle, type FitnessBand, type FitnessIndexEntry, type ModelRow, type SlotView } from './fitness'

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
  // FLEET LAST, and present at all only since fleet slots existed. Twelve
  // harnesses — the work session, the channel plan, both briefers, all three
  // Inbox harnesses — were measured and archived into a matrix with no column
  // to show them in, because their model is the subject of the call rather than
  // anything an admin assigns from a registry. See `SlotKind` in score.ts.
  const fleet = $derived(slots.filter((s) => s.kind === 'fleet'))
  const ordered = $derived([...roles, ...agents, ...fleet])

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

<!-- Sixteen columns never fit a 4xl page: the table scrolls inside its own
     box (UI-CONVENTIONS: wide content scrolls itself, the page body never
     scrolls sideways) and the model column is sticky so a row stays readable. -->
<div class="overflow-x-auto rounded-lg border border-line">
  <table class="w-full border-collapse text-left">
    <thead>
      <!-- The header is its own BAND — opaque `bg-card` rather than the panel
           behind it, closed by a strong hairline. A 15-column grid of 5px
           glyphs needs its axes to read as axes; before this the head was the
           same surface as the body in the dimmest ink on the palette. -->
      <tr class="border-b border-line-strong">
        <th class="sticky left-0 z-10 bg-card px-3 py-2 font-mono text-[11px] uppercase tracking-[0.08em] text-muted">Model</th>
        <!-- SPEED SITS BEFORE THE SLOTS, not after them. It is a fact about the
             MODEL rather than about any one slot — like the safety band — and it
             is the second thing an admin reads after the name, so putting it at
             the far end of sixteen columns of glyphs would hide it behind a
             horizontal scroll. Measured over the same fixtures for every
             candidate, which is the only reason a column of these is comparable
             at all. -->
        <th
          class="border-l border-line bg-card px-2 py-2 align-bottom font-mono text-[11px] font-normal tracking-[0.04em] text-muted"
          title="Output tokens per second, median across the fixtures this model ran. A rate rather than a duration, so it is comparable between models that ran different fixtures. Hover a cell for latency and the width it was measured at."
        >
          <span class="block min-h-28 [text-orientation:mixed] [writing-mode:vertical-rl] whitespace-nowrap text-end">Speed</span>
        </th>
        {#each ordered as slot, i (slot.key)}
          <th
            class={cn(
              'bg-card px-1 py-2 align-bottom font-mono text-[11px] font-normal tracking-[0.04em] text-muted',
              // A hairline down every column INCLUDING THE FIRST: the eye
              // tracks one slot through thirteen rows of near-identical dots,
              // and a grid is the only thing that makes that possible at this
              // density. The first rule also closes the model column, which
              // otherwise ran straight into the first slot with nothing
              // between a model id and a verdict about it.
              'border-l border-line',
              // The one seam that MEANS something — roles are the activity
              // classes, platform agents are Talaria's own workers — so it is a
              // step stronger than the column rules around it.
              i > 0 && ordered[i - 1]?.kind !== slot.kind && 'border-l-line-strong',
              // The seam between "about the model" and "about a slot".
              i === 0 && 'border-l-line-strong',
            )}
            title="{slot.label} — {slot.hint}{slot.requires.length ? `\nNeeds: ${slot.requires.join(', ')}` : ''}"
          >
            <!-- Vertical column heads: 15 horizontal labels would set the
                 table's width from its header text rather than its data.

                 MIN-HEIGHT, NOT HEIGHT. `h-28` was a flat 112px against labels
                 that run to "Workbench · Standard effort" — about 160px of
                 vertical text — so the longest heads overflowed the header cell
                 and printed down across the first rows of the matrix. Vertical
                 writing mode has no ellipsis to fall back on, so the fix is to
                 let the header row size itself to its longest label: every head
                 keeps a common floor, and the one long one makes the row taller
                 instead of escaping it.

                 BOTTOM-ALIGNED, AND IT TAKES BOTH RULES. `align-bottom` on the
                 cell only sinks the SPAN; inside it the text still started at
                 the inline-start edge, so a short label floated at the top of
                 its 112px floor while the long one filled 160px — the labels
                 raggedly ended wherever their own length ran out. In
                 `vertical-rl` the inline axis runs top→bottom, so `text-end` is
                 the bottom, and every label now ends flush against the header
                 rule with the growth going upward. -->
            <!-- No "(reserved)" suffix: `slotViews` no longer sends a slot no
                 harness reaches, so every column here is one a run can speak
                 about. See its comment for the two reasons a slot has no
                 column, and why one of them was never "reserved". -->
            <span class="block min-h-28 [text-orientation:mixed] [writing-mode:vertical-rl] whitespace-nowrap text-end">
              {slot.label}
            </span>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as m (m.id)}
        {@const entry = index[m.id]}
        {@const summary = rowSummary(entry, ordered)}
        <!-- `group` so the hover reaches the STICKY cell too: it carries its
             own opaque background (it has to — the body scrolls under it), so
             a row-level `hover:dither-fill` stopped dead at the model name and the
             highlight that helps you track a row across fifteen columns was
             missing from the one column you read it from.

             `border-line`, not `border-line-subtle`: subtle is #232019 against
             a #141312 panel, which is a rule you cannot see. -->
        <tr
          class={cn('group border-b border-line transition-colors last:border-0 hover:dither-fill', selected === m.id && 'bg-card')}
        >
          <!-- THE WHOLE CELL IS THE TARGET. The button used to be a shrink-wrapped
               box around the text, so the dead space beside a short model id —
               most of a 22rem column — looked clickable (the row highlights) and
               was not. The padding moved onto the button so the hit area is the
               cell. -->
          <th
            scope="row"
            class={cn('sticky left-0 z-10 p-0 font-normal transition-colors', selected === m.id ? 'bg-card' : 'bg-panel group-hover:dither-fill')}
          >
            <!-- THE ROW OPENS THE REPORT, and it has to look like it does. This
                 was a bare button with no hover state of its own inside a row
                 that highlights on hover, so the whole row lit up and nothing
                 said WHICH part was the door. A dotted underline that sharpens
                 on hover, plus an arrow that only appears then: legible without
                 adding a fourth colour to a page already dense with them. -->
            <button
              type="button"
              onclick={() => onSelect(m.id)}
              title="Open {m.id} — per-slot verdicts, capabilities and every failing fixture"
              class={cn('group/row flex w-full max-w-[22rem] cursor-pointer flex-col items-start gap-0.5 px-3 py-1.5 text-left', focusGold)}
            >
              <span class="flex min-w-0 items-baseline gap-1.5">
                <span
                  class="truncate font-mono text-xs text-fg underline decoration-dotted decoration-line underline-offset-2 transition-colors group-hover/row:decoration-accent"
                >
                  {m.id}
                </span>
                <span class="shrink-0 font-mono text-[10px] text-ink-dim opacity-0 transition-opacity group-hover/row:opacity-100">open →</span>
              </span>
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
                    title="Adversarial (tier 3): {SAFETY_META[entry.safety.band].label}. {SAFETY_META[entry.safety.band]
                      .blurb} This is the MODEL ALONE — Talaria's guardrails are a second layer that runs on top of it in production, and the adversarial pane shows what they would have caught."
                  >
                    · safety {SAFETY_META[entry.safety.band].label.toLowerCase()}{entry.safety.resistance === null
                      ? ''
                      : ` ${Math.round(entry.safety.resistance * 100)}%`}
                  </span>
                {/if}
              </span>
              <CapabilityTags row={m} negativeOnly />
            </button>
          </th>
          <!-- THE SPEED CELL. A number, not a band: there is no threshold that
               is right for both a frontier API and a 7B on one GPU, and a made-up
               one would be the page asserting a judgement nobody made. Every row
               ran the same fixtures, so the COLUMN is the comparison. -->
          <td class="border-l border-line px-2 py-1.5 text-center">
            {#if entry?.speed}
              <span class="font-mono text-[11px] text-fg" title={speedTitle(entry.speed)}>
                {entry.speed.tokensPerSecond === null ? ms(entry.speed.p50) : `${entry.speed.tokensPerSecond} t/s`}
              </span>
              {#if entry.speed.concurrency > 1}
                <!-- The caveat, visible rather than only in the tooltip: two
                     medians measured at different widths are not comparable, and
                     the column invites exactly that comparison. -->
                <span class="block font-mono text-[9px] text-ink-dim" title={speedTitle(entry.speed)}>×{entry.speed.concurrency}</span>
              {/if}
            {:else}
              <span class="font-mono text-[11px] text-ink-dim" title={speedTitle(null)}>·</span>
            {/if}
          </td>
          {#each ordered as slot, i (slot.key)}
            {@const band = bandOf(entry, slot.key)}
            <td
              class={cn(
                'px-1 py-1.5 text-center',
                // The same grid the header rules — see the note there.
                'border-l border-line',
                i > 0 && ordered[i - 1]?.kind !== slot.kind && 'border-l-line-strong',
              // The seam between "about the model" and "about a slot".
              i === 0 && 'border-l-line-strong',
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
</div>
