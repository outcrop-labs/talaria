import { describe, expect, it } from 'vitest'
import { joinFrontmatter, splitFrontmatter } from './frontmatter'

const SKILL = '---\nname: load-matching\ndescription: Rank carriers for a load.\n---\n\n# Load matching\n\nWhen to use: a load has no carrier.\n'

const roundTrips: Array<[string, string]> = [
  ['no frontmatter', '# Load matching\n\nWhen to use: a load has no carrier.\n'],
  ['normal frontmatter', SKILL],
  ['--- inside a quoted value', '---\nname: x\ndescription: "before --- after"\n---\n\nBody.\n'],
  ['a quoted value wrapped onto a line that starts with ---', '---\ndescription: "first\n--- second"\n---\nBody.\n'],
  ['a line of four dashes inside the block', '---\nname: x\n----\nmore: y\n---\nBody.\n'],
  ['Windows line endings', '---\r\nname: x\r\ndescription: y\r\n---\r\n\r\n# Title\r\n\r\nBody.\r\n'],
  ['an empty body', '---\nname: x\n---\n'],
  ['an empty body and no final newline', '---\nname: x\n---'],
  ['an empty block', '---\n---\nBody.\n'],
  ['a body that starts with ---', '---\nname: x\n---\n---\n\nAfter the rule.\n'],
  ['a body that starts with --- after a blank line', '---\nname: x\n---\n\n---\nAfter the rule.\n'],
  ['no frontmatter, a body that opens with a rule', '---\n\nA rule opened this document.\n'],
  ['trailing whitespace on the fences', '--- \nname: x\n---  \t\n\nBody.\n'],
  ['trailing whitespace and no final newline', '---\nname: x  \n---\n\nBody with spaces after it.   '],
  ['several blank lines before the body', '---\nname: x\n---\n\n\n\nBody.\n'],
  ['an opening fence that never closes', '---\nname: x\nBody with no closing fence.\n'],
  ['only whitespace', ' \n\n'],
  ['empty', ''],
]

describe('frontmatter: the document survives being split and joined', () => {
  it.each(roundTrips)('%s', (_name, text) => {
    expect(joinFrontmatter(splitFrontmatter(text))).toBe(text)
  })
  it.each(roundTrips)('%s: the three parts are the text, in order', (_name, text) => {
    const { head, gap, body } = splitFrontmatter(text)
    expect(head + gap + body).toBe(text)
  })
})

describe('splitFrontmatter', () => {
  it('lifts the block and leaves the body without its leading blank lines', () => {
    expect(splitFrontmatter(SKILL)).toEqual({
      head: '---\nname: load-matching\ndescription: Rank carriers for a load.\n---',
      gap: '\n\n',
      body: '# Load matching\n\nWhen to use: a load has no carrier.\n',
    })
  })
  it('finds no block in a document without one', () => {
    expect(splitFrontmatter('# Title\n\n---\n\nA rule further down.\n')).toEqual({ head: '', gap: '', body: '# Title\n\n---\n\nA rule further down.\n' })
  })
  it('closes the block only at a line that is a fence', () => {
    expect(splitFrontmatter('---\ndescription: "first\n--- second"\n---\nBody.\n').head).toBe('---\ndescription: "first\n--- second"\n---')
    expect(splitFrontmatter('---\nname: x\n----\nmore: y\n---\nBody.\n').head).toBe('---\nname: x\n----\nmore: y\n---')
  })
  it('keeps a rule that opens the body in the body', () => {
    expect(splitFrontmatter('---\nname: x\n---\n---\n\nAfter the rule.\n').body).toBe('---\n\nAfter the rule.\n')
  })
})

describe('joinFrontmatter: an edited body or block', () => {
  it('keeps the block byte for byte when only the body changed', () => {
    const parts = splitFrontmatter(SKILL)
    const saved = joinFrontmatter({ ...parts, body: parts.body.replace('carrier.', 'truck.') })
    expect(saved.startsWith('---\nname: load-matching\ndescription: Rank carriers for a load.\n---\n\n')).toBe(true)
    expect(saved).toBe(SKILL.replace('carrier.', 'truck.'))
  })
  it('puts a line break between a block that ended the document and a body typed under it', () => {
    expect(joinFrontmatter({ ...splitFrontmatter('---\nname: x\n---'), body: 'New body.\n' })).toBe('---\nname: x\n---\nNew body.\n')
    expect(joinFrontmatter({ ...splitFrontmatter('---\r\nname: x\r\n---'), body: 'New body.' })).toBe('---\r\nname: x\r\n---\r\nNew body.')
  })
  it('writes a retyped block in place of the old one, in the document’s own line endings', () => {
    const unix = splitFrontmatter(SKILL)
    expect(joinFrontmatter(unix, '---\nname: load-matching\ndescription: Match loads.\n---\n')).toBe(
      '---\nname: load-matching\ndescription: Match loads.\n---\n\n# Load matching\n\nWhen to use: a load has no carrier.\n',
    )
    const windows = splitFrontmatter('---\r\nname: x\r\n---\r\n\r\nBody.\r\n')
    expect(joinFrontmatter(windows, '---\nname: y\n---')).toBe('---\r\nname: y\r\n---\r\n\r\nBody.\r\n')
  })
  it('drops a block that was emptied', () => {
    expect(joinFrontmatter(splitFrontmatter(SKILL), '  \n')).toBe('# Load matching\n\nWhen to use: a load has no carrier.\n')
  })
})
