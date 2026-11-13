<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { caseCategory, type HealthSummary, type Suspicion } from './fitness'

  // WHICH OF OUR FIXTURES ARE BROKEN — the question the matrix cannot answer.
  //
  // A red cell says a model failed a fixture. It cannot say whether the fixture
  // is right, and those are different problems with different owners. For a
  // month the only way to tell them apart was for somebody to notice the same
  // assertion failing on a second model and go and read it by hand — which is
  // how every fixture rewritten this round was found.
  //
  // THIS PANEL DOES NOT JUDGE. It counts, groups the reasons verbatim, and says
  // how many models agreed. "A hard task" and "a wrong assertion" look identical
  // in the counts; what separates them is reading the sentence, so the sentence
  // is what it puts in front of you.
  let { data }: { data: HealthSummary } = $props()

  const TONE: Record<Suspicion, { label: string; tone: 'danger' | 'warn' | 'neutral' | 'accent'; blurb: string }> = {
    ours: {
      label: 'Ours',
      tone: 'warn',
      blurb:
        'Every model that ran this fixture came back wrong, or the fixture itself reported it could not fairly ask its question. Read the assertion before you read the models.',
    },
    shared: {
      label: 'Most models',
      tone: 'warn',
      blurb: 'More than half of the models tested failed this. Could be a genuinely hard task; could be an assertion only the strongest model satisfies.',
    },
    model: { label: 'One model', tone: 'neutral', blurb: 'One candidate of several failed this. That is what a fitness suite is for.' },
    unknown: {
      label: 'Needs a second model',
      tone: 'neutral',
      blurb: 'Fewer than two models have reached a verdict on this fixture, so nothing can be concluded from it yet.',
    },
  }

  let filter = $state<Suspicion | 'all'>('all')
  const shown = $derived(data.fixtures.filter((f) => filter === 'all' || f.suspicion === filter))
  const counts = $derived({
    ours: data.fixtures.filter((f) => f.suspicion === 'ours').length,
    shared: data.fixtures.filter((f) => f.suspicion === 'shared').length,
    model: data.fixtures.filter((f) => f.suspicion === 'model').length,
    unknown: data.fixtures.filter((f) => f.suspicion === 'unknown').length,
  })

  type Filter = Suspicion | 'all'
  const filters = $derived<SegmentedOption<Filter>[]>([
    { id: 'all', label: `All ${data.fixtures.length}` },
    { id: 'ours', label: `Ours ${counts.ours}`, title: TONE.ours.blurb },
    { id: 'shared', label: `Most models ${counts.shared}`, title: TONE.shared.blurb },
    { id: 'model', label: `One model ${counts.model}`, title: TONE.model.blurb },
    { id: 'unknown', label: `Needs a second model ${counts.unknown}`, title: TONE.unknown.blurb },
  ])

  // One fixture open at a time is not the point here: comparing two fixtures'
  // reasons side by side is, so each row keeps its own state.
  let opened = $state<Record<string, boolean>>({})
  const keyOf = (f: { harness: string; case: string }) => `${f.harness}::${f.case}`
</script>

{#if data.models.length === 0}
  <EmptyState icon="◇" title="No archived runs" hint="Test a model and its fixtures appear here." />
{:else}
  <div class="space-y-4">
    <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
      Every fixture that went wrong on at least one of the {data.models.length} model{data.models.length === 1 ? '' : 's'} on record, and how many
      of them agreed.
      {#if data.models.length < 2}
        <span class="text-warning">
          Only one model has been tested, so nothing here can be attributed yet. A fixture is only suspicious when a second, unrelated model fails
          it the same way.
        </span>
      {/if}
    </p>

    <Segmented options={filters} value={filter} onChange={(id) => (filter = id)} />

    {#if shown.length === 0}
      <EmptyState
        variant="compact"
        icon="◇"
        title={filter === 'ours' ? 'No fixture failed on every model' : 'Nothing in this group'}
        hint={filter === 'ours'
          ? 'That is the number to keep at zero. It counts assertions no model could satisfy, which are almost always ours and not theirs.'
          : 'The counts above say where the rest are.'}
      />
    {:else}
      <SettingsGroup>
        {#each shown as f (keyOf(f))}
          {@const open = opened[keyOf(f)] === true}
          <div>
            <button
              type="button"
              onclick={() => (opened = { ...opened, [keyOf(f)]: !open })}
              aria-expanded={open}
              class="flex min-h-[60px] w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
            >
              <ChevronRight size={16} class={cn('shrink-0 text-muted transition-transform duration-150', open && 'rotate-90')} aria-hidden="true" />
              <span class="min-w-0 flex-1">
                <span class="block truncate font-sans text-sm font-medium leading-5 text-fg">{f.case}</span>
                <span class="block truncate font-sans text-[13px] leading-5 text-muted">
                  {caseCategory(f.harness).label} · <span class="font-mono text-xs">{f.harness}</span>
                </span>
              </span>
              {#if f.gapped > 0}<Chip tone="warn" title="The fixture reported it could not fairly ask its question. That is a statement about the harness.">Gap ×{f.gapped}</Chip>{/if}
              {#if f.timedOut > 0}<Chip tone="warn">Timed out ×{f.timedOut}</Chip>{/if}
              {#if f.unmeasured > 0}<Chip tone="neutral" title="Skipped, or the provider never answered. Not a verdict either way.">Unmeasured ×{f.unmeasured}</Chip>{/if}
              <span class="shrink-0 font-sans text-[13px] leading-5 tabular-nums text-muted">{f.failed} of {f.tested} failed</span>
              <Chip tone={TONE[f.suspicion].tone} title={TONE[f.suspicion].blurb}>{TONE[f.suspicion].label}</Chip>
            </button>
            {#if open}
              <div transition:slide={{ duration: 150 }}>
                <div class="space-y-3 border-t border-line-subtle py-4 pl-[52px] pr-5">
                  <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-muted">{TONE[f.suspicion].blurb}</p>
                  <!-- THE SENTENCES, VERBATIM AND GROUPED. Two models failing
                       for the SAME stated reason is a far stronger signal than
                       two models failing, and a paraphrase here would destroy
                       exactly that. -->
                  {#each f.reasons as r (r.reason)}
                    <div>
                      <p class="max-w-[72ch] font-sans text-[13px] leading-5 text-fg">{r.reason}</p>
                      <p class="font-mono text-xs leading-5 text-muted">{r.models.join(' · ')}</p>
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
          </div>
        {/each}
      </SettingsGroup>
    {/if}
  </div>
{/if}
