<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import CodeBlock from '@/components/ui/CodeBlock.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import type { EvalCaseScore } from './fitness'

  // THE DRILL-DOWN, and it is what makes a red cell trustworthy rather than
  // merely alarming: the fixture's own one-line assertion, the ACTUAL prompt
  // that was sent, and the ACTUAL reply that came back.
  //
  // `EvalCaseScore.prompt` / `.raw` exist for exactly this and are populated
  // only for cases that failed something — a clean run carries neither, because
  // seventy passing transcripts in a settings row is an archive, not telemetry.
  let {
    cases,
    dropped = 0,
    harnessLabels = {},
  }: { cases: EvalCaseScore[]; dropped?: number; harnessLabels?: Record<string, string> } = $props()

  // Worth reading, in the order an admin should read it: a case with no reply
  // at all (a refused floor or a dead gateway) explains itself and everything
  // after it, so it goes first.
  //
  // A SKIPPED CASE IS NOT A FAILED ASSERTION and is filtered out before any of
  // this. It has `contractHeld: false` — the same zero every unmeasured field
  // carries — so it used to land at the top of this list with a "no answer"
  // chip, 0ms, 0/0 tokens and no transcript to open, once per fixture. That is
  // the loudest possible way to say nothing. The reason is stated ONCE below
  // instead, which is how many times it is true.
  const failing = $derived(
    cases
      .filter((c) => c.skipped === null)
      .filter((c) => !c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null || c.findings > 0)
      .sort((a, b) => Number(a.answered) - Number(b.answered)),
  )

  /** One line per DISTINCT reason, with the fixtures it cost. */
  const skipped = $derived.by(() => {
    const by = new Map<string, number>()
    for (const c of cases) {
      if (c.skipped !== null) by.set(c.skipped, (by.get(c.skipped) ?? 0) + 1)
    }
    return [...by].map(([reason, n]) => ({ reason, n }))
  })

  const label = (c: EvalCaseScore): string => `${harnessLabels[c.harness] ?? c.harness} · ${c.case}`
</script>

<!-- STATED BEFORE THE FAILURES, because it changes how they read: an admin who
     does not know two harnesses were never tested will read the rest as the
     whole picture. It is never a pass — the wording says so. -->
{#if skipped.length > 0}
  <ul class="mb-3 space-y-1">
    {#each skipped as s (s.reason)}
      <li class="max-w-prose font-sans text-xs text-warning">
        {s.n} fixture{s.n === 1 ? '' : 's'} not run: {s.reason}
      </li>
    {/each}
  </ul>
{/if}

{#if failing.length === 0}
  <EmptyState
    variant="compact"
    icon="◇"
    title="Nothing failed"
    hint={cases.length === 0
      ? 'This run recorded no fixture cases.'
      : skipped.length > 0
        ? 'Every fixture that ran held its contract and passed its own check. The ones above did not run.'
        : 'Every recorded fixture held its contract and passed its own check.'}
  />
{:else}
  <div class="space-y-2">
    {#each failing as c (`${c.harness}::${c.case}`)}
      <Disclosure>
        {#snippet title()}
          <span class="flex items-center gap-2">
            <span class="truncate font-mono text-[11px] text-fg">{label(c)}</span>
            {#if c.timedOut}<Chip tone="warn">timed out</Chip>{/if}
            {#if !c.answered}<Chip tone="neutral" title="No reply to apply the contract to — a refused capability floor, a chain that routed nothing, or a transport that died. That is the run, not the model.">no answer</Chip>{/if}
            {#if c.answered && !c.contractHeld}<Chip tone="danger">contract failed</Chip>{/if}
            {#if c.contractHeld && c.repairs > 0}<Chip tone="warn" title="The first reply was not valid; a repair turn recovered it. On a small model this is the difference between usable and not.">repaired ×{c.repairs}</Chip>{/if}
            {#if c.task === 'fail'}<Chip tone="danger">check failed</Chip>{/if}
            {#if c.findings > 0}<Chip tone="danger" title="A guard rule fired on an ORDINARY fixture. That is a safety regression, not a task score.">{c.findings} finding{c.findings === 1 ? '' : 's'}</Chip>{/if}
            {#if c.optimistic}<Chip tone="warn" title="The contract held and the fixture rejected the value anyway. Either the fixture grades quality the contract deliberately does not police, or the contract is recording a value the caller will throw away.">optimistic</Chip>{/if}
          </span>
        {/snippet}
        <div class="space-y-3 p-3">
          <!-- THE ASSERTION, VERBATIM. `EvalCase.check` is documented to write
               its reason for the admin reading this panel, so it is printed as
               written rather than summarized into a rate. -->
          {#if c.taskError}
            <p class="max-w-prose font-sans text-xs text-warning">{c.taskError}</p>
          {/if}
          {#if c.error}
            <p class="max-w-prose font-sans text-xs text-danger">{c.error}</p>
          {/if}
          <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            {c.latencyMs}ms · {c.promptTokens}/{c.completionTokens} tok{c.estimated ? ' (estimated)' : ''}
          </div>
          {#if c.prompt}
            <div>
              <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Prompt sent</div>
              <CodeBlock code={c.prompt} />
            </div>
          {/if}
          {#if c.raw}
            <div>
              <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Reply received</div>
              <CodeBlock code={c.raw} />
            </div>
          {:else if c.answered}
            <p class="font-sans text-xs text-muted">No transcript was kept for this case.</p>
          {/if}
        </div>
      </Disclosure>
    {/each}
    {#if dropped > 0}
      <p class="font-sans text-xs text-muted">
        {dropped} further case{dropped === 1 ? '' : 's'} were not kept — transcripts are bounded so the archive stays a drill-down rather than
        a log. Re-run this harness alone to see them.
      </p>
    {/if}
  </div>
{/if}
