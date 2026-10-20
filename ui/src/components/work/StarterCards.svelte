<script lang="ts">
  // THE FOUR WAYS IN, on the empty stage. The catalogue is
  // `starter-cards.ts`; this is the rendering and nothing else, so what a card
  // DOES stays the harness's business (it owns the agent, the composer and the
  // research ask).
  //
  // A card is a Panel with the button inside it rather than around it: the
  // button is full-bleed, so the whole card is the target, and the Panel keeps
  // its own surface and its ambient field. `field="ambient"` because these are
  // merely prominent — nothing here is SELECTED (UI-CONVENTIONS: a surface
  // where everything has texture says the same as one where nothing does).
  import Panel from '@/components/ui/Panel.svelte'
  import { focusRing } from '@/components/ui/control'
  import { cn } from '@/lib/cn'
  import { STARTER_CARDS, type StarterCard } from './starter-cards'

  let {
    onStart,
    disabledReason = null,
  }: {
    onStart: (card: StarterCard) => void
    /** Why none of these can be started right now, in the words the tooltip
     *  says. NULL is the working state. It is a REASON rather than a boolean
     *  because a disabled control with no tooltip is a dead end: the harness
     *  knows the answer ("Pick an agent first") and this is the only place to
     *  put it. */
    disabledReason?: string | null
  } = $props()

  const off = $derived(!!disabledReason)
</script>

<div class="mx-auto w-full max-w-[var(--converse-width)] px-4 pb-6">
  <div class="mb-2 px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Start with</div>
  <div class="grid gap-3 sm:grid-cols-2">
    {#each STARTER_CARDS as card (card.key)}
      <Panel field="ambient" class="p-0">
        <button
          type="button"
          disabled={off}
          title={disabledReason ?? undefined}
          onclick={() => onStart(card)}
          class={cn(
            'block w-full rounded-lg p-4 text-left transition-colors dither-fill disabled:cursor-default disabled:opacity-50',
            focusRing,
          )}
        >
          <div class="text-sm text-fg">{card.title}</div>
          <!-- Sans: a hint is a sentence somebody READS. -->
          <p class="mt-1 font-sans text-xs leading-relaxed text-muted">{card.hint}</p>
        </button>
      </Panel>
    {/each}
  </div>
</div>
