import { describe, expect, it } from 'vitest'

import { entryTitle, joinEntries, removeEntry, replaceEntry, searchEntries, splitEntries } from './memory-entries'

// THE CONTRACT UNDER TEST IS A FILE FORMAT, and the cost of getting it wrong
// is not cosmetic: Hermes' memory tool re-reads MEMORY.md before every write
// and refuses to write at all once the file stops round-tripping as a clean
// §-delimited list. So the assertions that matter are the round trip (what we
// write is what we read), the normalising (a file somebody hand-edited comes
// back clean WITHOUT losing text), and the one false positive that would cut
// an entry in half — a `§` used as a section sign inside a sentence.

const FILE = 'Jon prefers short replies.\n§\nShips on Fridays, reviews on Mondays.\n'

describe('splitEntries / joinEntries: the on-disk shape', () => {
  it('round-trips a clean file byte for byte', () => {
    expect(splitEntries(FILE)).toEqual(['Jon prefers short replies.', 'Ships on Fridays, reviews on Mondays.'])
    expect(joinEntries(splitEntries(FILE))).toBe(FILE)
  })

  it('round-trips a multi-line entry, keeping its internal blank lines', () => {
    const file = 'Deploy notes:\n\n- rc first\n- then main\n§\nOne fact.\n'
    expect(splitEntries(file)).toEqual(['Deploy notes:\n\n- rc first\n- then main', 'One fact.'])
    expect(joinEntries(splitEntries(file))).toBe(file)
  })

  it('normalises stray blank lines around the delimiter without losing text', () => {
    const messy = '\n\nJon prefers short replies.\n\n§\n\n  Ships on Fridays, reviews on Mondays.  \n\n'
    expect(splitEntries(messy)).toEqual(['Jon prefers short replies.', 'Ships on Fridays, reviews on Mondays.'])
    expect(joinEntries(splitEntries(messy))).toBe(FILE)
  })

  it('treats a marker line with stray whitespace as the delimiter it plainly is', () => {
    expect(splitEntries('one\n §  \ntwo')).toEqual(['one', 'two'])
  })

  it('does NOT split on a section sign used inside a line', () => {
    const file = 'The policy is in §4 of the contract — quote it, do not paraphrase.\n§\nTwo.\n'
    const entries = splitEntries(file)
    expect(entries).toHaveLength(2)
    expect(entries[0]).toBe('The policy is in §4 of the contract — quote it, do not paraphrase.')
    // A lone `§` mid-sentence is text too, as long as it is not the whole line.
    expect(splitEntries('a § b')).toEqual(['a § b'])
    expect(joinEntries(splitEntries(file))).toBe(file)
  })

  it('normalises CRLF, consecutive delimiters and empty entries away', () => {
    expect(splitEntries('one\r\n§\r\ntwo\r\n')).toEqual(['one', 'two'])
    expect(splitEntries('one\n§\n§\n\n§\ntwo')).toEqual(['one', 'two'])
    expect(splitEntries('§\none\n§\n')).toEqual(['one'])
  })

  it('reads an empty or marker-only file as an empty list, and writes one back as an empty file', () => {
    for (const file of ['', '\n\n', '§', '\n§\n']) expect(splitEntries(file)).toEqual([])
    expect(joinEntries([])).toBe('')
    expect(joinEntries(['', '   ', '\n'])).toBe('')
  })

  it('keeps the ending the file had', () => {
    expect(joinEntries(['one', 'two'])).toBe('one\n§\ntwo\n')
    expect(joinEntries(['one', 'two'], false)).toBe('one\n§\ntwo')
    expect(joinEntries(splitEntries('one\n§\ntwo'), false)).toBe('one\n§\ntwo')
  })

  it('writes a clean list from entries a person typed loosely', () => {
    // What the inline editor and the add box hand back: untrimmed text, a
    // blank one among them. The file must still read back as what was kept.
    const written = joinEntries(['  one  ', '', '\ntwo\n'])
    expect(written).toBe('one\n§\ntwo\n')
    expect(splitEntries(written)).toEqual(['one', 'two'])
  })
})

describe('searchEntries: positions, not copies', () => {
  const entries = ['Jon prefers short replies', 'Ships on Fridays', 'Reviews on MONDAYS']

  it('matches case-insensitively anywhere in the entry', () => {
    expect(searchEntries(entries, 'mondays')).toEqual([2])
    expect(searchEntries(entries, 'on')).toEqual([0, 1, 2])
    expect(searchEntries(entries, 'SHIPS')).toEqual([1])
  })

  it('matches everything on a blank query and nothing on a miss', () => {
    expect(searchEntries(entries, '')).toEqual([0, 1, 2])
    expect(searchEntries(entries, '   ')).toEqual([0, 1, 2])
    expect(searchEntries(entries, 'tuesday')).toEqual([])
    expect(searchEntries([], 'anything')).toEqual([])
  })

  it('returns indices into the ORIGINAL list, so a write after a filter hits the right entry', () => {
    const hits = searchEntries(entries, 'mondays')
    expect(hits).toEqual([2])
    // `?? -1` only to satisfy the index check: an out-of-range index would
    // leave the list untouched, and the assertion below would catch that.
    expect(replaceEntry(entries, hits[0] ?? -1, 'Reviews on Tuesdays')).toEqual([
      'Jon prefers short replies',
      'Ships on Fridays',
      'Reviews on Tuesdays',
    ])
  })
})

describe('entryTitle: one line, clipped', () => {
  it('takes the first line that says something', () => {
    expect(entryTitle('\n\nShips on Fridays\nand reviews on Mondays')).toBe('Ships on Fridays')
    expect(entryTitle('')).toBe('')
  })

  it('drops markdown furniture', () => {
    expect(entryTitle('## Preferences\n\nShort replies.')).toBe('Preferences')
    expect(entryTitle('- likes dark mode _(added by hand, 2026-10-16)_')).toBe('likes dark mode _(added by hand, 2026-10-16)_')
  })

  it('clips long lines INSIDE the budget, marker included', () => {
    const long = `${'a'.repeat(120)} tail`
    const title = entryTitle(long)
    expect(title).toHaveLength(80)
    expect(title.endsWith('…')).toBe(true)
    // Exactly at the budget, nothing is clipped and no marker appears.
    const exact = 'b'.repeat(80)
    expect(entryTitle(exact)).toBe(exact)
  })
})

describe('replaceEntry / removeEntry: new lists, never a mutation', () => {
  const entries = ['one', 'two', 'three']

  it('replaces by index without touching the input', () => {
    expect(replaceEntry(entries, 1, '  TWO  ')).toEqual(['one', 'TWO', 'three'])
    expect(entries).toEqual(['one', 'two', 'three'])
  })

  it('treats an entry edited down to nothing as a removal', () => {
    // A blank entry in the list is the exact shape that stops the agent
    // writing to its own memory, so it must not be representable.
    expect(replaceEntry(entries, 1, '   \n ')).toEqual(['one', 'three'])
    expect(joinEntries(replaceEntry(entries, 1, ''))).toBe('one\n§\nthree\n')
  })

  it('removes by index without touching the input', () => {
    expect(removeEntry(entries, 0)).toEqual(['two', 'three'])
    expect(removeEntry(entries, 2)).toEqual(['one', 'two'])
    expect(entries).toEqual(['one', 'two', 'three'])
  })

  it('changes nothing for an index nothing lives at', () => {
    for (const i of [-1, 3, 99]) {
      expect(replaceEntry(entries, i, 'x')).toEqual(entries)
      expect(removeEntry(entries, i)).toEqual(entries)
    }
  })
})

describe('joinEntries — a marker line inside an entry', () => {
  it('is written escaped so the next read does not split the entry', () => {
    const entries = ['first line\n§\nstill the first entry', 'second']
    const out = joinEntries(entries)
    expect(splitEntries(out)).toHaveLength(2)
    expect(splitEntries(out)[0]).toContain('still the first entry')
  })
})
