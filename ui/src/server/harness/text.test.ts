import { describe, expect, it } from 'vitest'
import { firstMeaningfulLine } from './text'
import { cleanTitle } from './defs/titler'

// The three shapes below are why this helper exists: each one used to be STORED
// rather than rejected, and a stored artifact survives until the content hash
// changes. They are written as the model actually emits them.
describe('firstMeaningfulLine', () => {
  it('takes the bare answer unchanged', () => {
    expect(firstMeaningfulLine('Writes release notes from merged PRs.')).toBe('Writes release notes from merged PRs.')
  })

  it('unwraps a fenced answer instead of storing the fence', () => {
    expect(firstMeaningfulLine('```\nWrites release notes from merged PRs.\n```')).toBe('Writes release notes from merged PRs.')
    expect(firstMeaningfulLine('```markdown\nSprint 14 planning\n```')).toBe('Sprint 14 planning')
  })

  it('strips a trailing bold marker, not only a leading one', () => {
    expect(firstMeaningfulLine('**Writes release notes**')).toBe('Writes release notes')
  })

  it('skips a lead-in when an answer follows it', () => {
    expect(firstMeaningfulLine("Here's the summary:\n\nWrites release notes from merged PRs.")).toBe('Writes release notes from merged PRs.')
  })

  it('keeps a one-line answer that merely ends in a colon', () => {
    expect(firstMeaningfulLine('Deploys, in three steps:')).toBe('Deploys, in three steps:')
  })

  // The colon form was already skipped; this is the same preamble without one,
  // which is how a small model most often ignores "reply with ONLY the title".
  // Every one of these used to be STORED — as a chat's name, as a skill's
  // subtitle — and a 3-7 word apology passes every assertion `titleProblem`
  // makes, so the model scored green on the job it was failing.
  it('skips a lead-in that does not end in a colon', () => {
    const answer = 'Checkout Latency on Mobile'
    for (const lead of [
      "Sure, here's a good title",
      'Understood. I will return the title.',
      'Here is the title.',
      'Okay, here is a concise title',
      'Based on the conversation, the title is',
      "Absolutely, here's a fitting title",
      'I will name it as follows.',
    ]) {
      expect(firstMeaningfulLine(`${lead}\n\n${answer}`), lead).toBe(answer)
    }
  })

  it('keeps a one-line answer that merely opens with one of those words', () => {
    // Nothing follows, so it is the answer rather than a lead-in.
    expect(firstMeaningfulLine('Here is the plan')).toBe('Here is the plan')
    expect(firstMeaningfulLine('Based on the transcript')).toBe('Based on the transcript')
  })

  it('strips a list marker the model put in front of its one-line answer', () => {
    expect(firstMeaningfulLine('- Checkout latency on mobile')).toBe('Checkout latency on mobile')
    expect(firstMeaningfulLine('1. Checkout latency on mobile')).toBe('Checkout latency on mobile')
    expect(firstMeaningfulLine('2) Checkout latency on mobile')).toBe('Checkout latency on mobile')
    expect(firstMeaningfulLine('• Checkout latency on mobile')).toBe('Checkout latency on mobile')
    // Not a list: no space after the marker.
    expect(firstMeaningfulLine('-5 degrees and falling')).toBe('-5 degrees and falling')
  })

  it('unwraps bold around PART of the line instead of leaving the closing marker', () => {
    // The half-stripped shape ("Checkout Latency** on Mobile") is worse than no
    // strip: it is asymmetric, so nothing downstream can remove it, and the
    // summarizer's own noMarkdown fixture then scores the MODEL for residue the
    // shared cleaner created.
    expect(firstMeaningfulLine('**Checkout Latency** on Mobile')).toBe('Checkout Latency on Mobile')
    expect(firstMeaningfulLine('**Writes release notes** from PRs.')).toBe('Writes release notes from PRs.')
    expect(firstMeaningfulLine('Reads `SKILL.md` and posts')).toBe('Reads SKILL.md and posts')
  })

  it('strips quotes and heading markers', () => {
    expect(firstMeaningfulLine('## "Checkout latency on mobile"')).toBe('Checkout latency on mobile')
  })

  it('returns null when nothing survives', () => {
    expect(firstMeaningfulLine('')).toBeNull()
    expect(firstMeaningfulLine('\n\n   \n')).toBeNull()
    expect(firstMeaningfulLine('```\n```')).toBeNull()
    expect(firstMeaningfulLine('***')).toBeNull()
  })
})

describe('cleanTitle', () => {
  it('keeps a trailing hash, which is content rather than a closing marker', () => {
    expect(cleanTitle('Sprint 14 planning #3')).toBe('Sprint 14 planning #3')
  })

  it('drops the trailing period the prompt forbade', () => {
    expect(cleanTitle('Checkout latency on mobile.')).toBe('Checkout latency on mobile')
    expect(cleanTitle('決済の遅延。')).toBe('決済の遅延')
  })

  it('clamps a model that ignored "3-7 words" to one sidebar row', () => {
    const value = cleanTitle('word '.repeat(60))
    expect(value).not.toBeNull()
    expect((value as string).length).toBeLessThanOrEqual(91)
    expect(value).toMatch(/…$/)
  })
})
