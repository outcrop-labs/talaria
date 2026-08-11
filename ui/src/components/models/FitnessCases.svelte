<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import CodeBlock from '@/components/ui/CodeBlock.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { cn } from '@/lib/cn'
  import { caseCategory, type EvalCaseScore } from './fitness'

  // THE DRILL-DOWN, and it is what makes a red cell trustworthy rather than
  // merely alarming: the fixture's own one-line assertion, the ACTUAL prompt
  // that was sent, the ACTUAL reply that came back — and, for a case that ran a
  // tool loop, EVERY TURN AND EVERY TOOL CALL in between.
  //
  // WHY THE TRANSCRIPT MATTERS MORE THAN THE VERDICT. A behavioural fixture's
  // reason is one sentence: "never called get_ticket, so it commented on a
  // ticket it had not read". That sentence is either an indictment of the model
  // or an indictment of our harness, and NOTHING ON THIS PAGE COULD TELL YOU
  // WHICH until the turns were here. Half the fixtures rewritten this round were
  // rewritten because the transcript showed we had not given the model what the
  // job needed — a channel id it had no way to look up, a closing question asked
  // of a run that had not finished. A verdict you cannot audit is a verdict you
  // end up arguing with.
  //
  // TABS ARE BY CATEGORY, not by outcome, and the outcome is the filter inside
  // them. "Show me everything that failed" is one click either way; "how did
  // this model do on Research" is only answerable this way round, and it is the
  // question an admin picking a model per role actually has.
  //
  // `fill` MAKES IT A SELF-CONTAINED PANE: header pinned, list scrolling inside
  // it, nothing escaping. The category bar used to be `position: sticky`, which
  // only works when the component and its scroll container agree about who is
  // scrolling — and in the Live view they did not, so the bar escaped its
  // section and the failures list drew over the console beneath it. A pinned
  // header inside a bounded box needs no such agreement.
  let {
    cases,
    dropped = 0,
    harnessLabels = {},
    fill = false,
  }: { cases: EvalCaseScore[]; dropped?: number; harnessLabels?: Record<string, string>; fill?: boolean } = $props()

  type Outcome = 'failing' | 'gaps' | 'passing' | 'skipped'

  /** WHICH BUCKET A CASE IS IN, decided once so the tab counts and the list can
   *  never disagree. A skipped case is checked FIRST: it carries
   *  `contractHeld: false` like every unmeasured field, and reading that as a
   *  failure is exactly the mistake this order prevents. */
  const outcomeOf = (c: EvalCaseScore): Outcome => {
    if (c.skipped !== null) return 'skipped'
    // OUR GAP, NOT THE MODEL'S FAILURE. The fixture could not fairly ask its
    // question, so this is a bug report about the harness — kept in its own
    // bucket rather than mixed into the failures, because the fix is ours.
    if (c.gap !== null) return 'gaps'
    if (!c.contractHeld || c.task === 'fail' || c.timedOut || c.error !== null || c.findings > 0) return 'failing'
    return 'passing'
  }

  const scored = $derived(cases.map((c) => ({ c, outcome: outcomeOf(c), category: caseCategory(c.harness) })))

  const categories = $derived.by(() => {
    const by = new Map<string, { id: string; label: string; n: number; bad: number }>()
    for (const s of scored) {
      const at = by.get(s.category.id) ?? { id: s.category.id, label: s.category.label, n: 0, bad: 0 }
      at.n++
      if (s.outcome === 'failing') at.bad++
      by.set(s.category.id, at)
    }
    // Worst first. An admin opening this wants what would bite them, not an
    // alphabetical tour — the same ordering rule the slot list uses.
    return [...by.values()].sort((a, b) => b.bad - a.bad || b.n - a.n || a.label.localeCompare(b.label))
  })

  let tab = $state<string>('all')
  let outcome = $state<Outcome | 'all'>('failing')

  // A tab that stops existing (a live sweep's first harness finishing, a filter
  // emptying it) must not leave the list showing nothing with no way back.
  $effect(() => {
    if (tab !== 'all' && !categories.some((c) => c.id === tab)) tab = 'all'
  })

  const inTab = $derived(scored.filter((s) => tab === 'all' || s.category.id === tab))
  const counts = $derived({
    failing: inTab.filter((s) => s.outcome === 'failing').length,
    gaps: inTab.filter((s) => s.outcome === 'gaps').length,
    passing: inTab.filter((s) => s.outcome === 'passing').length,
    skipped: inTab.filter((s) => s.outcome === 'skipped').length,
  })

  const shown = $derived(
    inTab
      .filter((s) => outcome === 'all' || s.outcome === outcome)
      // Within a bucket: no answer at all first, since a case with no reply
      // explains itself and everything after it.
      .sort((a, b) => Number(a.c.answered) - Number(b.c.answered)),
  )

  // The default view is the one worth opening on, but "Failing" on a clean run
  // is an empty panel that reads like a bug. So the filter follows the run.
  $effect(() => {
    if (outcome === 'failing' && counts.failing === 0 && counts.gaps > 0) outcome = 'gaps'
    else if (outcome === 'failing' && counts.failing === 0 && counts.passing > 0) outcome = 'passing'
  })

  const tabItems = $derived([
    { id: 'all', label: `All ${scored.length}` },
    ...categories.map((c) => ({ id: c.id, label: c.bad > 0 ? `${c.label} ${c.bad}✕` : `${c.label} ${c.n}` })),
  ])

  const filterOptions = $derived(
    [
      { id: 'failing' as const, label: `Failing ${counts.failing}`, title: 'The contract broke, the fixture rejected the value, the case timed out, or a guard rule fired.' },
      { id: 'gaps' as const, label: `Our gap ${counts.gaps}`, title: 'The fixture could not fairly ask its question — the run was never given what the assertion demanded. A bug report about the harness, not a score about the model.' },
      { id: 'passing' as const, label: `Passed ${counts.passing}`, title: 'The contract held and the fixture accepted the value.' },
      { id: 'skipped' as const, label: `Not run ${counts.skipped}`, title: 'The sweep never called the model. Not a pass.' },
      { id: 'all' as const, label: `All ${inTab.length}` },
    ].filter((o) => o.id === 'all' || o.id === outcome || (counts as Record<string, number>)[o.id]! > 0),
  )

  const label = (c: EvalCaseScore): string => `${harnessLabels[c.harness] ?? c.harness} · ${c.case}`

  /** One line per DISTINCT skip reason, said once rather than once per fixture. */
  const skipReasons = $derived.by(() => {
    const by = new Map<string, number>()
    for (const s of scored) if (s.c.skipped !== null) by.set(s.c.skipped, (by.get(s.c.skipped) ?? 0) + 1)
    return [...by].map(([reason, n]) => ({ reason, n }))
  })

  const ROLE_TONE: Record<string, string> = {
    system: 'text-ink-dim',
    user: 'text-muted',
    assistant: 'text-fg',
    tool: 'text-accent',
  }

  const pretty = (json: string): string => {
    try {
      return JSON.stringify(JSON.parse(json), null, 1)
    } catch {
      return json
    }
  }
</script>

<div class={fill ? 'flex h-full min-h-0 flex-col gap-3' : 'space-y-3'}>
  <!-- STATED BEFORE THE FAILURES, because it changes how they read: an admin who
       does not know two harnesses were never tested will read the rest as the
       whole picture. It is never a pass — the wording says so. -->
  {#if skipReasons.length > 0}
    <ul class="space-y-1">
      {#each skipReasons as s (s.reason)}
        <li class="max-w-prose font-sans text-xs text-warning">
          {s.n} fixture{s.n === 1 ? '' : 's'} not run: {s.reason}
        </li>
      {/each}
    </ul>
  {/if}

  {#if scored.length === 0}
    <EmptyState variant="compact" icon="◇" title="No fixture cases" hint="This run recorded none." />
  {:else}
    <!-- PINNED, NOT STICKY. The category tabs are how you get back out of a list
         of forty fixtures, so they must not scroll away — but `sticky` escapes
         its section the moment the component and its scroll container disagree
         about who is scrolling, which is how this bar came to be drawn over the
         console. In `fill` mode it is simply a `shrink-0` row above a scrolling
         sibling, which cannot escape anything. -->
    <div class={cn('flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-line-subtle pb-2', fill && 'shrink-0')}>
      <Tabs items={tabItems} value={tab} onChange={(id) => (tab = id)} class="flex-wrap" />
      <Segmented class="ml-auto" options={filterOptions} value={outcome} onChange={(id) => (outcome = id)} />
    </div>

    <!-- THE ONLY THING THAT SCROLLS in `fill` mode, and it scrolls inside its own
         box. Two components that each own a bounded box cannot draw over each
         other, whatever either of them does internally. -->
    <div class={fill ? 'min-h-0 flex-1 overflow-y-auto' : ''}>
    {#if shown.length === 0}
      <EmptyState
        variant="compact"
        icon="◇"
        title="Nothing here"
        hint="No case in this category has that outcome. The counts above say where the rest are."
      />
    {:else}
      <div class="space-y-2">
        {#each shown as s (`${s.c.harness}::${s.c.case}`)}
          {@const c = s.c}
          <Disclosure>
            {#snippet title()}
              <span class="flex flex-wrap items-center gap-2">
                <span class="truncate font-mono text-[11px] text-fg">{label(c)}</span>
                {#if c.skipped !== null}<Chip tone="neutral" title={c.skipped}>not run</Chip>{/if}
                {#if c.timedOut}<Chip tone="warn">timed out</Chip>{/if}
                {#if c.skipped === null && !c.answered}<Chip
                    tone="neutral"
                    title="No reply to apply the contract to — a refused capability floor, a chain that routed nothing, or a transport that died. That is the run, not the model."
                    >no answer</Chip
                  >{/if}
                {#if c.answered && !c.contractHeld}<Chip tone="danger">contract failed</Chip>{/if}
                {#if c.contractHeld && c.repairs > 0}<Chip
                    tone="warn"
                    title="The first reply was not valid; a repair turn recovered it. On a small model this is the difference between usable and not."
                    >repaired ×{c.repairs}</Chip
                  >{/if}
                {#if c.gap !== null}<Chip tone="warn" title="The fixture could not fairly ask its question. This is a bug report about our harness, not a score about the model."
                    >our gap</Chip
                  >{/if}
                {#if c.task === 'fail'}<Chip tone="danger">check failed</Chip>{/if}
                {#if c.task === 'pass'}<Chip tone="success">passed</Chip>{/if}
                {#if c.findings > 0}<Chip tone="danger" title="A guard rule fired on an ORDINARY fixture. That is a safety regression, not a task score."
                    >{c.findings} finding{c.findings === 1 ? '' : 's'}</Chip
                  >{/if}
                {#if c.optimistic}<Chip
                    tone="warn"
                    title="The contract held and the fixture rejected the value anyway. Either the fixture grades quality the contract deliberately does not police, or the contract is recording a value the caller will throw away."
                    >optimistic</Chip
                  >{/if}
                <!-- THE HEADLINE NUMBER FOR A TOOL-LOOP CASE. "12 calls" next to
                     "never moved the ticket" is a different story from "0 calls"
                     next to the same sentence, and both are read at a glance. -->
                {#if c.calls && c.calls.length > 0}
                  <span class="font-mono text-[10px] text-ink-dim">
                    {c.calls.length} call{c.calls.length === 1 ? '' : 's'}{#if c.calls.some((x) => x.error)}<span class="text-danger"
                        >, {c.calls.filter((x) => x.error).length} refused</span
                      >{/if}
                  </span>
                {/if}
              </span>
            {/snippet}

            <div class="space-y-3 p-3">
              {#if c.skipped !== null}
                <p class="max-w-prose font-sans text-xs text-muted">{c.skipped}</p>
              {/if}
              <!-- THE ASSERTION, VERBATIM. `EvalCase.check` is documented to write
                   its reason for the admin reading this panel, so it is printed as
                   written rather than summarized into a rate. -->
              {#if c.gap}
                <p class="max-w-prose font-sans text-xs text-warning">
                  <span class="font-mono text-[10px] uppercase tracking-[0.08em]">our gap</span> — {c.gap}
                </p>
              {/if}
              {#if c.taskError}
                <p class="max-w-prose font-sans text-xs text-warning">{c.taskError}</p>
              {/if}
              {#if c.error}
                <p class="max-w-prose font-sans text-xs text-danger">{c.error}</p>
              {/if}
              <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                {c.band} · {c.latencyMs}ms · {c.promptTokens}/{c.completionTokens} tok{c.estimated ? ' (estimated)' : ''}
              </div>

              <!-- WHAT THE BUDGET WAS ACTUALLY SPENT ON. A timeout used to say
                   only that it had happened, which cannot tell a slow model from
                   a request that never came back from four retries from time
                   that never reached the provider at all. -->
              {#if c.upstream && c.upstream.length > 0}
                <div>
                  <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Upstream calls</div>
                  <ol class="space-y-0.5">
                    {#each c.upstream as u, i (i)}
                      <li class="flex flex-wrap items-baseline gap-2 font-mono text-[11px]">
                        <span class="text-ink-dim">{i + 1}</span>
                        <span class={!u.settled ? 'text-warning' : u.error ? 'text-danger' : 'text-muted'}>{u.ms}ms</span>
                        {#if !u.settled}
                          <span class="text-warning">no reply — still open when the case was killed</span>
                        {:else if u.error}
                          <span class="min-w-0 flex-1 break-words text-danger">{u.error}</span>
                        {:else}
                          <span class="text-ink-dim">ok</span>
                        {/if}
                      </li>
                    {/each}
                  </ol>
                </div>
              {:else if c.timedOut}
                <p class="max-w-prose font-sans text-xs text-warning">
                  This case timed out having made no upstream call at all — the time went somewhere before the request reached the provider.
                </p>
              {/if}

              <!-- WHAT IT ACTUALLY DID, before what it said. Every behavioural
                   fixture asserts over this list and nothing else, so it is the
                   first thing to read when one of them fails. Kept for passing
                   cases too: comparing two models on one fixture IS comparing
                   these two lists. -->
              {#if c.calls && c.calls.length > 0}
                <div>
                  <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Tool calls, in order</div>
                  <ol class="space-y-1">
                    {#each c.calls as call, i (i)}
                      <li class="rounded-md border border-line-subtle bg-raised/40 px-2 py-1.5">
                        <div class="flex flex-wrap items-baseline gap-2">
                          <span class="font-mono text-[10px] text-ink-dim">{i + 1}</span>
                          <span class={cn('font-mono text-[11px]', call.error ? 'text-danger' : 'text-fg')}>{call.tool}</span>
                          {#if call.error}<Chip tone="danger" title="The tool refused, exactly as production would. A refusal is a real event — the fixture may require one.">refused</Chip>{/if}
                        </div>
                        <pre class="mt-1 overflow-x-auto font-mono text-[10px] leading-4 text-muted">{pretty(call.args)}</pre>
                        {#if call.error}
                          <p class="mt-1 font-sans text-[11px] text-danger">{call.error}</p>
                        {:else if call.result}
                          <pre class="mt-1 max-h-32 overflow-auto font-mono text-[10px] leading-4 text-ink-dim">{pretty(call.result)}</pre>
                        {/if}
                      </li>
                    {/each}
                  </ol>
                </div>
              {:else if c.calls}
                <p class="font-sans text-xs text-muted">This case ran a tool loop and the model called nothing at all.</p>
              {/if}

              <!-- THE WHOLE CONVERSATION. Present only for a case that took more
                   than one turn; a single-shot harness's entire story is the
                   prompt and the reply below. -->
              {#if c.turns && c.turns.length > 0}
                <Disclosure title="Turn history ({c.turns.length} messages)">
                  <ol class="space-y-2 p-1">
                    {#each c.turns as t, i (i)}
                      <li>
                        <div class="mb-0.5 flex items-baseline gap-2">
                          <span class={cn('font-mono text-[10px] uppercase tracking-[0.08em]', ROLE_TONE[t.role] ?? 'text-muted')}>{t.role}</span>
                          {#if t.toolCalls?.length}
                            <span class="font-mono text-[10px] text-accent">→ {t.toolCalls.join(', ')}</span>
                          {/if}
                        </div>
                        {#if t.content.trim()}
                          <pre class="max-h-48 overflow-auto whitespace-pre-wrap font-mono text-[11px] leading-4 text-muted">{t.content}</pre>
                        {:else}
                          <!-- A turn that only calls tools says nothing, and that
                               is normal — labelling it makes the transcript read
                               as the tool conversation it was. -->
                          <p class="font-sans text-[11px] text-ink-dim">(no prose — the turn was tool calls)</p>
                        {/if}
                      </li>
                    {/each}
                  </ol>
                </Disclosure>
              {/if}

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
              {:else if c.answered && c.task !== 'pass'}
                <p class="font-sans text-xs text-muted">No transcript was kept for this case.</p>
              {:else if c.task === 'pass'}
                <p class="font-sans text-xs text-muted">
                  Transcripts are kept for cases that failed something — seventy passing ones in a settings row is an archive, not telemetry. What
                  it DID is above.
                </p>
              {/if}
            </div>
          </Disclosure>
        {/each}
      </div>
    {/if}

    </div>
    {#if dropped > 0}
      <p class={cn('font-sans text-xs text-muted', fill && 'shrink-0')}>
        {dropped} further case{dropped === 1 ? '' : 's'} were not kept — transcripts are bounded so the archive stays a drill-down rather than a
        log. Re-run this harness alone to see them.
      </p>
    {/if}
  {/if}
</div>
