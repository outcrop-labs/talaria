// envfile semantics: first-match wins, values may contain `=`, one quote
// layer strips, env shadows file, secrets land 0600.

import { describe, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { envValue, envWins, parseEnv, stripQuotes, writeSecret } from './envfile'

describe('parseEnv / envValue', () => {
  const text = [
    '# comment',
    '',
    'A=1',
    'A=second-wins-no  # parseEnv keeps the FIRST like grep -m1',
    'B=base64==',
    'C=hello world',
    'not a var line',
    'QUOTED="wrapped"',
  ].join('\n')

  test('first matching line wins; value is everything after the first =', () => {
    expect(envValue(text, 'A')).toBe('1')
    expect(envValue(text, 'B')).toBe('base64==')
    expect(envValue(text, 'C')).toBe('hello world')
    expect(envValue(text, 'MISSING')).toBeUndefined()
  })

  test('parseEnv matches the same first-wins rule', () => {
    const v = parseEnv(text)
    expect(v.A).toBe('1')
    expect(v.B).toBe('base64==')
    expect(v.QUOTED).toBe('"wrapped"')
  })

  test('stripQuotes removes exactly one layer', () => {
    expect(stripQuotes('"a"')).toBe('a')
    expect(stripQuotes("'a'")).toBe('a')
    expect(stripQuotes('"a')).toBe('"a')
    expect(stripQuotes('a"b')).toBe('a"b')
  })
})

describe('envWins', () => {
  test('set env vars shadow the file; unset ones do not', () => {
    expect(envWins({ A: 'file', B: 'file' }, { A: 'env', B: undefined })).toEqual({
      A: 'env',
      B: 'file',
    })
  })
})

describe('writeSecret', () => {
  test('writes 0600, and re-chmods a pre-existing wider file', () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-cli-'))
    const p = join(dir, 'compose.env')
    writeSecret(p, 'BOX_NAME=x\n')
    expect(statSync(p).mode & 0o777).toBe(0o600)
    expect(readFileSync(p, 'utf8')).toBe('BOX_NAME=x\n')
    // existed with 0644 → still ends 0600
    writeFileSync(p, 'old\n', { mode: 0o644 })
    writeSecret(p, 'new\n')
    expect(statSync(p).mode & 0o777).toBe(0o600)
  })
})
