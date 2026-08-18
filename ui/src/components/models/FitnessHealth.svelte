<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { cn } from '@/lib/cn'
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
      tone: 'danger',
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
</script>

{#if data.models.length === 0}
  <EmptyState icon="◇" title="No archived runs" hint="Test a model and its fixtures appear here." />
{:else}
  <div class="space-y-3">
    <p class="max-w-prose font-sans text-xs text-muted">
      Every fixture that went wrong on at least one of the {data.models.length} model{data.models.length === 1 ? '' : 's'} on record, and how many
      of them agreed.
      {#if data.models.length < 2}
        <span class="text-warning">
          Only one model has been tested, so nothing here can be attributed yet — a fixture is only suspicious when a second, unrelated model fails
          it the same way.
        </span>
      {/if}
    </p>

    <div class="flex flex-wrap items-center gap-1.5">
      {#each [['all', `All ${data.fixtures.length}`], ['ours', `Ours ${counts.ours}`], ['shared', `Most models ${counts.shared}`], ['model', `One model ${counts.model}`], ['unknown', `Unattributable ${counts.unknown}`]] as [id, label] (id)}
        <button
          type="button"
          onclick={() => (filter = id as Suspicion | 'all')}
          class={cn(
            'rounded-md border px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
            filter === id ? 'border-line bg-raised text-fg' : 'border-transparent text-muted dither-fill hover:text-fg',
          )}
        >
          {label}
        </button>
      {/each}
    </div>

    {#if shown.length === 0}
      <EmptyState
        variant="compact"
        icon="◇"
        title={filter === 'ours' ? 'No fixture failed on every model' : 'Nothing in this bucket'}
        hint={filter === 'ours'
          ? 'That is the number to keep at zero: it counts assertions that no model could satisfy, which are almost always ours rather than theirs.'
          : 'The counts above say where the rest are.'}
      />
    {:else}
      <ul class="space-y-2">
        {#each shown as f (`${f.harness}::${f.case}`)}
          <li>
            <Disclosure>
              {#snippet title()}
                <span class="flex flex-wrap items-center gap-2">
                  <Chip tone={TONE[f.suspicion].tone} title={TONE[f.suspicion].blurb}>{TONE[f.suspicion].label}</Chip>
                  <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">{caseCategory(f.harness).label}</span>
                  <span class="truncate font-mono text-[11px] text-muted">{f.harness}</span>
                  <span class="min-w-0 flex-1 truncate text-fg">{f.case}</span>
                  <span class="font-mono text-[10px] text-muted">{f.failed}/{f.tested} failed</span>
                  {#if f.gapped > 0}<Chip tone="warn" title="The fixture reported it could not fairly ask its question. That is a statement about the harness.">gap ×{f.gapped}</Chip>{/if}
                  {#if f.timedOut > 0}<Chip tone="warn">timeout ×{f.timedOut}</Chip>{/if}
                  {#if f.unmeasured > 0}<Chip tone="neutral" title="Skipped, or the provider never answered. Not a verdict either way.">unmeasured ×{f.unmeasured}</Chip>{/if}
                </span>
              {/snippet}
              <div class="space-y-2 p-3">
                <p class="max-w-prose font-sans text-xs text-muted">{TONE[f.suspicion].blurb}</p>
                <!-- THE SENTENCES, VERBATIM AND GROUPED. Two models failing for
                     the SAME stated reason is a far stronger signal than two
                     models failing, and a paraphrase here would destroy exactly
                     that. -->
                {#each f.reasons as r (r.reason)}
                  <div class="rounded-md border border-line-subtle bg-raised/40 px-2.5 py-1.5">
                    <p class="font-sans text-xs text-warning">{r.reason}</p>
                    <p class="mt-0.5 font-mono text-[10px] text-ink-dim">{r.models.join(' · ')}</p>
                  </div>
                {/each}
              </div>
            </Disclosure>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}
