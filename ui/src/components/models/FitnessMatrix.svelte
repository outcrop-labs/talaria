<script lang="ts">
  import { cn } from '@/lib/cn'
  import CapabilityTags from './CapabilityTags.svelte'
  import FitnessMark from './FitnessMark.svelte'
  import {
    SAFETY_META,
    BAND_META,
    BAND_TEXT,
    bandOf,
    ms,
    reasonOf,
    rowSummary,
    rowSummaryLine,
    safetyLine,
    slotHeading,
    speedTitle,
    type FitnessIndexEntry,
    type ModelRow,
    type SlotView,
  } from './fitness'

  // THE MATRIX. Rows = registered models, columns = the roles and platform
  // agents an admin can assign one to. This is the "can I swap this model in"
  // answer at a glance, and it is the feature.
  //
  // AN UNTESTED CELL LOOKS UNTESTED. A grey speck, a grey tooltip, and it is
  // the DEFAULT for every model with no archived run — the most dangerous
  // thing this page could do is imply a model was checked when it was not.
  let {
    slots,
    models,
    index,
    onSelect,
  }: {
    slots: SlotView[]
    models: ModelRow[]
    index: Record<string, FitnessIndexEntry>
    onSelect: (model: string) => void
  } = $props()

  const focus = 'focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent'

  const roles = $derived(slots.filter((s) => s.kind === 'role'))
  const agents = $derived(slots.filter((s) => s.kind === 'agent'))
  // FLEET LAST, and present at all only since fleet slots existed. Twelve
  // harnesses — the work session, the channel plan, both briefers, all three
  // Inbox harnesses — were measured and archived into a matrix with no column
  // to show them in, because their model is the subject of the call rather than
  // anything an admin assigns from a registry. See `SlotKind` in the Rust
  // fitness scorer (api/src/fitness/score.rs).
  const fleet = $derived(slots.filter((s) => s.kind === 'fleet'))
  // THE APPS' OWN HARNESSES, after everything the platform runs: one column
  // per harness an enabled app ships, scored on that app's own fixtures. They
  // come and go with the app, and they never move a platform column.
  const apps = $derived(slots.filter((s) => s.kind === 'app'))
  const ordered = $derived([...roles, ...agents, ...fleet, ...apps])

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
    return `${slot.label}: ${BAND_META[band].label}\n${reason ?? BAND_META[band].blurb}`
  }
</script>

<!-- A wide matrix scrolls inside its own card (UI-CONVENTIONS: wide content
     scrolls itself, the page body never scrolls sideways) and the model column
     is sticky so a row stays readable. -->
<div class="overflow-x-auto rounded-lg border border-line bg-panel">
  <table class="w-full border-collapse text-left">
    <thead>
      <tr class="border-b border-line">
        <th class="sticky left-0 z-10 bg-panel px-5 py-2.5 align-bottom font-sans text-xs font-medium leading-4 text-muted">Model</th>
        <!-- SPEED SITS BEFORE THE SLOTS, not after them. It is a fact about the
             MODEL rather than about any one slot, and it is the second thing
             an admin reads after the name, so the far end of a wide table
             would hide it behind a horizontal scroll. -->
        <th
          class="px-3 py-2.5 text-right align-bottom font-sans text-xs font-medium leading-4 text-muted"
          title="Output tokens per second, median across the fixtures this model ran. A rate rather than a duration, so it is comparable between models that ran different fixtures. Hover a cell for latency and the width it was measured at."
        >
          Speed
        </th>
        {#each ordered as slot (slot.key)}
          <th
            class="w-24 min-w-24 px-2 py-2.5 text-center align-bottom font-sans text-xs font-medium leading-4 text-muted"
            title="{slot.label}: {slot.hint}{slot.requires.length ? `\nNeeds: ${slot.requires.join(', ')}` : ''}"
          >
            {slotHeading(slot.label)}
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as m (m.id)}
        {@const entry = index[m.id]}
        {@const summary = rowSummary(entry, ordered)}
        <!-- `group` so the hover reaches the STICKY cell too: it carries its
             own opaque background (the body scrolls under it), so a row-level
             hover would stop dead at the model name. -->
        <tr class="group border-b border-line-subtle transition-colors last:border-0 hover:bg-hover">
          <!-- THE WHOLE CELL IS THE TARGET, and it opens the report. The
               padding sits on the button so the hit area is the cell. -->
          <th scope="row" class="sticky left-0 z-10 bg-panel p-0 font-normal transition-colors group-hover:bg-hover">
            <button
              type="button"
              onclick={() => onSelect(m.id)}
              title="Open {m.id} for per-slot verdicts, capabilities and every failing fixture"
              class={cn('flex w-full min-w-[18rem] cursor-pointer flex-col items-start gap-0.5 px-5 py-3 text-left', focus)}
            >
              <span class="flex min-w-0 max-w-full items-center gap-2">
                <span class="truncate font-mono text-[13px] leading-5 text-fg">{m.id}</span>
                <CapabilityTags row={m} negativeOnly />
              </span>
              <span class="whitespace-nowrap font-sans text-xs leading-4 text-muted">
                {entry ? rowSummaryLine(summary.counts) : 'Not tested yet'}
                <!-- Tier 3 is a fact about the MODEL, not about any one slot,
                     so it sits on the row and never colors a cell. -->
                {#if entry?.safety}
                  <span
                    class={entry.safety.band === 'ready' ? undefined : BAND_TEXT[entry.safety.band]}
                    title="Adversarial (tier 3): {SAFETY_META[entry.safety.band].label}. {SAFETY_META[entry.safety.band]
                      .blurb} This is the MODEL ALONE; Talaria's guardrails are a second layer that runs on top of it in production, and the adversarial pane shows what they would have caught."
                  >
                    · {safetyLine(entry.safety)}
                  </span>
                {/if}
              </span>
            </button>
          </th>
          <!-- THE SPEED CELL. A number, not a band: there is no threshold that
               is right for both a frontier API and a 7B on one GPU, and a made-up
               one would be the page asserting a judgement nobody made. Every row
               ran the same fixtures, so the COLUMN is the comparison. -->
          <td class="whitespace-nowrap px-3 py-3 text-right align-middle">
            {#if entry?.speed}
              <span class="block font-sans text-[13px] leading-5 tabular-nums text-fg" title={speedTitle(entry.speed)}>
                {entry.speed.tokensPerSecond === null ? ms(entry.speed.p50) : `${entry.speed.tokensPerSecond} t/s`}
              </span>
              {#if entry.speed.concurrency > 1}
                <!-- The caveat, visible rather than only in the tooltip: two
                     medians measured at different widths are not comparable, and
                     the column invites exactly that comparison. -->
                <span class="block font-sans text-xs leading-4 tabular-nums text-muted" title={speedTitle(entry.speed)}>
                  {entry.speed.concurrency} at once
                </span>
              {/if}
            {:else}
              <span class="font-sans text-[13px] text-muted" title={speedTitle(null)}>—</span>
            {/if}
          </td>
          {#each ordered as slot (slot.key)}
            {@const band = bandOf(entry, slot.key)}
            <td class="px-2 py-3 text-center align-middle">
              <button
                type="button"
                onclick={() => onSelect(m.id)}
                title={cellTitle(entry, slot)}
                aria-label="{m.id}, {slot.label}: {BAND_META[band].label}"
                class={cn('mx-auto grid h-7 w-7 place-items-center rounded-md transition-colors hover:bg-raised', focus)}
              >
                <FitnessMark {band} />
              </button>
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>
</div>
