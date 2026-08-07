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
  const failing = $derived(
    cases
      .filter((c) => !c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null || c.findings > 0)
      .sort((a, b) => Number(a.answered) - Number(b.answered)),
  )

  const label = (c: EvalCaseScore): string => `${harnessLabels[c.harness] ?? c.harness} · ${c.case}`
</script>

{#if failing.length === 0}
  <EmptyState
    variant="compact"
    icon="◇"
    title="Nothing failed"
    hint={cases.length === 0 ? 'This run recorded no fixture cases.' : 'Every recorded fixture held its contract and passed its own check.'}
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
