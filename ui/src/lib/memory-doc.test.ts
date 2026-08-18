import assert from 'node:assert/strict'
import { test } from 'vitest'
import { parseMemory } from './memory-doc'

const titles = (md: string) => parseMemory(md).map((e) => e.title)

test('an empty memory has no entries, which is not the same as one blank one', () => {
  assert.deepEqual(parseMemory(''), [])
  assert.deepEqual(parseMemory('   \n\n  '), [])
})

test('headings split the document, and each entry keeps its own heading', () => {
  const doc = `## Jon prefers terse buttons
He said so twice.

## The fleet runs on Arch
Migrated from Fedora.`
  const out = parseMemory(doc)
  assert.deepEqual(titles(doc), ['Jon prefers terse buttons', 'The fleet runs on Arch'])
  // The heading travels WITH the body: the detail pane renders the entry alone,
  // and a section without its own title reads as orphaned prose.
  assert.ok(out[0]!.body.startsWith('## Jon prefers terse buttons'))
  assert.ok(out[0]!.body.includes('He said so twice.'))
  assert.ok(!out[0]!.body.includes('Arch'))
})

test('prose before the first heading is kept, not silently dropped', () => {
  // Dropping it would lose real content from the picker — the reader would see
  // a shorter memory than the agent actually has.
  assert.deepEqual(titles('Some standing context.\n\n## A fact\nBody.'), ['Preamble', 'A fact'])
})

test('a leading heading produces no empty preamble', () => {
  assert.deepEqual(titles('## First\nBody.'), ['First'])
})

test('a bullet document splits per fact — the shape quick-add produces', () => {
  const doc = `- Jon migrated from Fedora to Arch _(added by hand, 2026-08-17)_
- The MCP port is 5280 _(added by hand, 2026-08-17)_`
  assert.deepEqual(titles(doc), ['Jon migrated from Fedora to Arch', 'The MCP port is 5280'])
})

test('the by-hand stamp is provenance, not content, so it leaves the label', () => {
  // It is the same length as some facts; leaving it in truncates the fact out
  // of the row it is supposed to identify.
  assert.deepEqual(titles('- Short fact _(added by hand, 2026-08-17)_'), ['Short fact'])
  // ...but it stays in the BODY, which is the record.
  assert.ok(parseMemory('- Short fact _(added by hand, 2026-08-17)_')[0]!.body.includes('added by hand'))
})

test('continuation lines belong to the bullet above them', () => {
  const out = parseMemory('- A fact\n  with more detail\n- Another')
  assert.equal(out.length, 2)
  assert.ok(out[0]!.body.includes('with more detail'))
})

test('mixed content prefers headings, because that is the curated shape', () => {
  // An agent that writes headings AND bullets means the bullets to sit under
  // the headings; splitting per bullet would shred a curated document.
  assert.deepEqual(titles('## Preferences\n- terse buttons\n- no view transitions'), ['Preferences'])
})

test('ids are unique even when two entries say the same thing', () => {
  const out = parseMemory('## Same\na\n\n## Same\nb')
  assert.equal(out.length, 2)
  assert.notEqual(out[0]!.id, out[1]!.id)
})

test('CRLF is read the same as LF', () => {
  assert.deepEqual(titles('## One\r\nbody\r\n\r\n## Two\r\nbody'), ['One', 'Two'])
})
