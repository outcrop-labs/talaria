import { describe, expect, it } from 'vitest'
import { harnessHealth, summarize, type HealthInput } from './health'
import type { EvalCaseScore } from './evals'

// THE ONE THING THIS FILE MUST NOT GET WRONG: calling a fixture ours when it is
// the model's, or the model's when it is ours. Both are worse than saying
// nothing — the first sends somebody to rewrite a working assertion, and the
// second is the status quo this report exists to replace.

const c = (over: Partial<EvalCaseScore>): EvalCaseScore =>
  ({
    harness: 'h',
    case: 'one',
    band: 'standard',
    skipped: null,
    contractHeld: true,
    firstPass: true,
    repairs: 0,
    answered: true,
    task: 'pass',
    taskError: null,
    gap: null,
    findings: 0,
    latencyMs: 10,
    startedAt: '2026-08-01T00:00:00.000Z',
    wallMs: 10,
    promptTokens: 0,
    completionTokens: 0,
    costUsd: null,
    estimated: false,
    timedOut: false,
    optimistic: false,
    error: null,
    prompt: null,
    raw: null,
    turns: null,
    calls: null,
    upstream: null,
    ...over,
  }) as EvalCaseScore

const run = (model: string, cases: EvalCaseScore[]): HealthInput => ({ model, cases })

describe('harnessHealth', () => {
  it('calls a fixture OURS when every model that ran it got it wrong', () => {
    // The signal that found the turn-budget bug: `work-session · hands a
    // finished ticket to review` failing on gemma AND deepseek. Two models with
    // nothing in common failing the same assertion for the same stated reason is
    // the assertion, not the models.
    const [f] = harnessHealth([
      run('gemma', [c({ task: 'fail', taskError: 'never handed the finished work to review' })]),
      run('deepseek', [c({ task: 'fail', taskError: 'never handed the finished work to review' })]),
    ])
    expect(f).toMatchObject({ harness: 'h', case: 'one', tested: 2, failed: 2, suspicion: 'ours' })
    // GROUPED VERBATIM, because two models failing for the SAME words is a much
    // stronger signal than two models failing.
    expect(f?.reasons).toEqual([{ reason: 'never handed the finished work to review', models: ['gemma', 'deepseek'] }])
  })

  it('calls it the MODEL when one of several got it wrong', () => {
    const [f] = harnessHealth([
      run('gemma', [c({ task: 'fail', taskError: 'lost the decision' })]),
      run('deepseek', [c({})]),
      run('claude', [c({})]),
    ])
    expect(f).toMatchObject({ tested: 3, failed: 1, suspicion: 'model' })
  })

  it('calls it SHARED when most but not all got it wrong', () => {
    const [f] = harnessHealth([
      run('a', [c({ task: 'fail', taskError: 'x' })]),
      run('b', [c({ task: 'fail', taskError: 'y' })]),
      run('c', [c({})]),
    ])
    expect(f?.suspicion).toBe('shared')
  })

  it('refuses to conclude anything from ONE model', () => {
    // "No evidence" and "the model's fault" are the two readings this whole file
    // exists to separate, so a single run says `unknown` rather than defaulting
    // to blaming the candidate.
    const [f] = harnessHealth([run('only', [c({ task: 'fail', taskError: 'x' })])])
    expect(f).toMatchObject({ tested: 1, failed: 1, suspicion: 'unknown' })
  })

  it('calls a GAP ours on the strength of one model alone', () => {
    // A gap is the fixture itself reporting that it could not fairly ask its
    // question. That is a statement about the harness, and no amount of
    // corroboration makes it more or less true.
    const [f] = harnessHealth([run('only', [c({ gap: 'the turn budget ran out', task: 'unscored' })]), run('other', [c({})])])
    expect(f).toMatchObject({ gapped: 1, suspicion: 'ours' })
  })

  it('counts a case the sweep could not measure apart from one it judged', () => {
    // A skip is not a verdict. Counting it as a failure would blame a model for
    // a harness its transport cannot drive, or for a provider that was busy.
    const [f] = harnessHealth([
      run('a', [c({ skipped: 'no tool loop on this candidate' })]),
      run('b', [c({ skipped: 'rate limits on every attempt' })]),
    ])
    expect(f).toMatchObject({ tested: 0, failed: 0, unmeasured: 2, suspicion: 'unknown' })
  })

  it('leaves the fixtures that always pass out of the report entirely', () => {
    // This is a list of things to fix. A green fixture on it is noise, and 247
    // of them would bury the four that matter.
    expect(harnessHealth([run('a', [c({})]), run('b', [c({})])])).toEqual([])
  })

  it('sorts ours first, then shared, then the model', () => {
    const out = harnessHealth([
      run('a', [c({ case: 'model-fault', task: 'fail', taskError: 'x' }), c({ case: 'everyone', task: 'fail', taskError: 'y' })]),
      run('b', [c({ case: 'model-fault' }), c({ case: 'everyone', task: 'fail', taskError: 'y' })]),
    ])
    expect(out.map((f) => f.case)).toEqual(['everyone', 'model-fault'])
  })
})

describe('summarize', () => {
  it('counts our own bugs as a number to work to zero', () => {
    const out = summarize([
      run('a', [c({ case: 'ours', task: 'fail', taskError: 'x' }), c({ case: 'theirs', task: 'fail', taskError: 'z' })]),
      run('b', [c({ case: 'ours', task: 'fail', taskError: 'x' }), c({ case: 'theirs' })]),
    ])
    expect(out).toMatchObject({ models: ['a', 'b'], ours: 1, shared: 0 })
  })
})
