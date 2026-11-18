import assert from 'node:assert/strict'
import { describe, test } from 'vitest'
import { userMentionInsert } from './mentions.svelte'

// The composer token for @mentioning a person. TALA-93: the picker inserted
// the RAW email localpart, but the server's content scanner stops a token at
// any char outside [a-z0-9-] — '@j.siegel' scanned to 'j' and could never
// match the member's token. The insert must speak the same normalized
// grammar the scanner reads: lowercase, every char outside [a-z0-9] mapped
// to '-', runs collapsed, edges trimmed.

describe('userMentionInsert — the token the composer inserts', () => {
  test('a dotted localpart normalizes to the scanner grammar', () => {
    assert.equal(userMentionInsert({ name: 'Jon Siegel', email: 'j.siegel@x.com' }), 'j-siegel')
  })

  test('an underscored localpart normalizes to the scanner grammar', () => {
    assert.equal(userMentionInsert({ name: 'Sam Underscore', email: 'sam_siegel@x.com' }), 'sam-siegel')
  })

  test('a plus-tagged localpart normalizes to the scanner grammar', () => {
    assert.equal(userMentionInsert({ name: 'Pat Plus', email: 'p.siegel+ops@x.com' }), 'p-siegel-ops')
  })

  test('a localpart already in the grammar passes through', () => {
    assert.equal(userMentionInsert({ name: 'Ada Lovelace', email: 'ada@x.com' }), 'ada')
  })

  test('no email falls back to the dashed full name', () => {
    assert.equal(userMentionInsert({ name: 'Grace Hopper', email: null }), 'grace-hopper')
  })
})
