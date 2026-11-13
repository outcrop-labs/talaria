import { describe, expect, it } from 'vitest'
import { clampWords, wordStops } from './word-clamp'

const within = (max: number) => (candidate: string) => candidate.length <= max

describe('wordStops', () => {
  it('stops after each whole word, with no punctuation left before the ellipsis', () => {
    expect(wordStops('Forge finished at 6:48 AM; 14 files changed, desktop only')).toEqual([
      'Forge…',
      'Forge finished…',
      'Forge finished at…',
      'Forge finished at 6:48…',
      'Forge finished at 6:48 AM…',
      'Forge finished at 6:48 AM; 14…',
      'Forge finished at 6:48 AM; 14 files…',
      'Forge finished at 6:48 AM; 14 files changed…',
      'Forge finished at 6:48 AM; 14 files changed, desktop…',
    ])
  })

  it('never stops on a dash, a bracket or a mark standing alone', () => {
    expect(wordStops('Jobs fail — all of them (double slash) today')).toEqual([
      'Jobs…',
      'Jobs fail…',
      'Jobs fail — all…',
      'Jobs fail — all of…',
      'Jobs fail — all of them…',
      'Jobs fail — all of them (double…',
      'Jobs fail — all of them (double slash)…',
    ])
  })

  it('has nowhere to stop in one word', () => {
    expect(wordStops('refs/heads/agent//help-7')).toEqual([])
  })
})

describe('clampWords', () => {
  const quote = '"Is anyone on HELP-6? I have a batch of 40 tickets to publish and the task API keeps 500ing."'

  it('leaves text that fits whole, with no ellipsis', () => {
    expect(clampWords(quote, within(200))).toBe(quote)
  })

  it('cuts to the longest run of whole words that fits, never inside one', () => {
    expect(clampWords(quote, within(49))).toBe('"Is anyone on HELP-6? I have a batch of 40…')
    expect(clampWords(quote, within(50))).toBe('"Is anyone on HELP-6? I have a batch of 40…')
    expect(clampWords(quote, within(51))).toBe('"Is anyone on HELP-6? I have a batch of 40 tickets…')
    expect(clampWords('needs a decision on the branch prefix format', within(19))).toBe('needs a decision…')
  })

  it('ends every cut on a letter or digit and the ellipsis, at any width', () => {
    for (let max = 8; max < quote.length; max++) expect(clampWords(quote, within(max))).toMatch(/^"Is(?: .*[\w)])?…$/)
  })

  it('hands back text whose first word does not fit, for its box to cut', () => {
    expect(clampWords('refs/heads/agent//help-7 is not a valid ref', within(10))).toBe('refs/heads/agent//help-7 is not a valid ref')
  })
})
