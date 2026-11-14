<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import StatusDot from '@/components/ui/StatusDot.svelte'
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

  // Every row has a verdict, so it is the row's status (a dot and a word)
  // and not a chip. The chips left on a row are the unusual things about it.
  const TONE: Record<Suspicion, { label: string; dot: DotStatus; blurb: string }> = {
    ours: {
      label: 'Ours',
      dot: 'warn',
      blurb:
        'Every model that ran this fixture came back wrong, or the fixture itself reported it could not fairly ask its question. Read the assertion before you read the models.',
    },
    shared: {
      label: 'Most models',
      dot: 'warn',
      blurb: 'More than half of the models tested failed this. Could be a genuinely hard task; could be an assertion only the strongest model satisfies.',
    },
    model: { label: 'One model', dot: 'idle', blurb: 'One candidate of several failed this. That is what a fitness suite is for.' },
    unknown: {
      label: 'Needs a second model',
      dot: 'idle',
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
            <SettingsRow label={f.case} onclick={() => (opened = { ...opened, [keyOf(f)]: !open })} expanded={open}>
              {#snippet hint()}{caseCategory(f.harness).label} · <span class="font-mono text-xs">{f.harness}</span>{/snippet}
              {#snippet status()}
                <span class="flex items-center gap-2" title={TONE[f.suspicion].blurb}><StatusDot status={TONE[f.suspicion].dot} />{TONE[f.suspicion].label}</span>
              {/snippet}
              {#if f.gapped > 0}<Chip tone="warn" title="The fixture reported it could not fairly ask its question. That is a statement about the harness.">Gap ×{f.gapped}</Chip>{/if}
              {#if f.timedOut > 0}<Chip tone="warn">Timed out ×{f.timedOut}</Chip>{/if}
              {#if f.unmeasured > 0}<Chip tone="neutral" title="Skipped, or the provider never answered. Not a verdict either way.">Unmeasured ×{f.unmeasured}</Chip>{/if}
              <span class="w-28 shrink-0 text-right font-sans text-[13px] leading-5 tabular-nums text-muted">{f.failed} of {f.tested} failed</span>
            </SettingsRow>
            {#if open}
              <div transition:slide={{ duration: 150 }}>
                <div class="space-y-3 border-t border-line-subtle px-5 py-4">
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
