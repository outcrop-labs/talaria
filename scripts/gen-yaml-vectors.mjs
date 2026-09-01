#!/usr/bin/env node
// Cross-language fixtures for the yaml emitter port (the npm `yaml` package ⇄
// api/src/yaml_string.rs). /api/history serves agent configs as
// `stringifyYaml(config)` and the TS bytes are the wire contract, so the Rust
// emitter is only trustworthy if it reproduces the package byte-for-byte.
// This script runs the REAL package over a case list that covers every
// grammar corner probed during the port — dispatch order, plain eligibility,
// core-scalar quoting, the three quoted forms, literal/folded blocks, the
// fold scanner (splits, hard chunks, fold-at-zero), chomping, markers,
// collection layout, and number formatting — and writes ONE committed file
// the Rust suite asserts against.
//
// The TS side of the contract is executed here, never re-derived: the values
// are plain JSON so the fixture carries both sides (input JSON, expected
// yaml text) and stays reproducible across machines.
//
// Usage:
//   bun run api:vectors:yaml           # write api/tests/fixtures/yaml-stringify.json
//   bun run api:vectors:yaml --check   # fail if the committed file is stale

import { createRequire } from 'node:module'
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const require = createRequire(new URL('../ui/package.json', import.meta.url))
const { stringify } = require('yaml')

const FIXTURE = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'api',
  'tests',
  'fixtures',
  'yaml-stringify.json'
)

const words = (n) => 'word '.repeat(n).trim()
const CASES = [
  // ── scalars: null / bool / numbers ──────────────────────────────────────
  ['null', null],
  ['true', true],
  ['false', false],
  ['int', 25],
  ['neg-int', -17],
  ['float', 0.1],
  ['float-2', 3.5],
  ['exp-small', 1e-7],
  ['exp-float', 1.5e-10],
  ['exp-big', 1e21],
  ['exp-mid', 1e5],
  ['big-precision', 123456789012345678901234567890],
  ['zero', 0],
  // -0 is unrepresentable here: JSON.stringify(-0) === '0', so the sign is
  // lost at generation. api/src/yaml_string.rs unit-tests it directly.
  ['num-root-plain', 100000],

  // ── collections: shape & nesting ────────────────────────────────────────
  ['empty-map', {}],
  ['empty-seq', []],
  ['null-val', { a: null }],
  ['all-null', { a: null, b: null }],
  ['mixed-null', { a: 1, b: null }],
  ['seq-null', { k: [null] }],
  ['empties-inline', { m: {}, s: [] }],
  ['seq-in-seq', [['x']]],
  ['seq-deep', [[['x']]]],
  ['seq-mixed-nest', ['a', ['b', ['c']]]],
  ['seq-of-map', [[{ a: 1 }]]],
  ['map-seq-under-key', { k: [[1]] }],
  ['map-nest-agent', { a: [{ b: ['c', { d: 0 }] }] }],
  ['single-pair-map', { y: 1 }],

  // ── strings: style dispatch & core-scalar quoting ───────────────────────
  ['empty', ''],
  ['space', ' '],
  ['plain', 'hello'],
  ['yes-stays-plain', 'yes'],
  ['true-quoted', 'True'],
  ['mixed-case-bool', 'tRue'],
  ['tilde', '~'],
  ['int-string', '123'],
  ['neg-int-string', '-42'],
  ['plus-int-string', '+42'],
  ['oct-string', '0o17'],
  ['bad-oct', '0o18'],
  ['hex-string', '0xFF'],
  ['hex-lower', '0xab'],
  ['binary-not-core', '0b101'],
  ['underscore-num', '1_000'],
  ['date-stays-plain', '2026-08-29'],
  ['time-colon', '3:30'],
  ['dot-inf', '.Inf'],
  ['neg-inf', '-.INF'],
  ['plus-inf', '+.inf'],
  ['nan', '.NaN'],
  ['neg-nan-not-core', '-.NaN'],
  ['dot-five', '.5'],
  ['five-dot', '5.'],
  ['exp-string', '1e3'],
  ['exp-signed', '+.1E5'],
  ['exp-neg', '-2.5e-3'],
  ['bare-dot', '.'],
  ['dash-x', '-x'],
  ['dash-alone', '-'],
  ['question-mark', '?'],
  ['dash-space', '- x'],
  ['bang-x', '!x'],
  ['amp-x', '&x'],
  ['at-x', '@x'],
  ['backtick-x', '`x'],
  ['percent-x', '%x'],
  ['hash-x', '#x'],
  ['comma-x', ',x'],
  ['bracket-x', '[x'],
  ['quote-x', "'x"],
  ['dquote-x', '"x'],
  ['pipe-x', '|x'],
  ['gt-x', '>x'],
  ['double-space', 'a  b'],
  ['interior-tab', 'a\tb'],
  ['colon-space', 'a: b'],
  ['colon-eol', 'a:'],
  ['colon-tab', 'a:\tb'],
  ['trail-colon', 'a:'],
  ['space-before-hash', 'a #c'],
  ['dash-plain-mid', 'a-b'],
  ['contains-quotes', "a: 'x'"],
  ['single-wins', 'say "hi"'],
  ['both-quotes', "it's \"it\""],
  ['neither-quote', 'plain value'],
  ['nbsp', ' x'],
  ['u2028', '\u2028x'],
  ['c1-raw', '\x85x'],
  ['del-raw', '\x7f'],
  ['ctl-1', '\x01ctl'],
  ['ctl-1b', '\x1b'],
  ['crlf', 'a\r\nb'],
  ['crlf-long', 'a\r' + 'y'.repeat(60) + '\n' + 'z'.repeat(60)],

  // ── keys ────────────────────────────────────────────────────────────────
  ['num-key', { 5: 1 }],
  ['empty-key', { '': 1 }],
  ['empty-key-nest', { '': { '': 1 } }],
  ['long-key-explicit', { ['k'.repeat(1025)]: 1 }],
  ['long-key-implicit-edge', { ['k'.repeat(1023)]: 1 }],
  ['long-key-fold0', { ['k'.repeat(61)]: words(30) }],
  ['quoted-key-value', { k: "a: 'b'" }],
  ['marker-key', { '%dir': 1 }],

  // ── blocks: literal ─────────────────────────────────────────────────────
  ['two-lines', 'line1\nline2'],
  ['trail-nl', 'trail\n'],
  ['keep-chomp', 'a\n\n'],
  ['blank-mid', 'a\n\nb'],
  ['blank-mid-nl', { k: 'a\n\nb\n' }],
  ['only-newlines', '\n\n'],
  ['quoted-line', 'a\n"b"'],
  ['tab-line', { k: 'a\n\tb\n' }],
  ['lead-space', ' x\ny'],
  ['lead-space-nl', ' \nx'],
  ['lead-space-k', { k: ' x\ny' }],
  ['marker-dashes', '----'],
  ['marker-dots', '...x'],
  ['marker-mid', 'a\n---x'],
  ['marker-mid-k', { k: 'a\n---x' }],
  ['marker-nested-plain', { k: '---x' }],
  ['percent-line', '%YAML 1.2\nx'],
  ['keep-nested', { k: 'a\n\n' }],
  ['trail-ws-mixed', 'a  \n\n'],
  ['trail-ws-line', 'a\n \n'],
  ['space-end-nl', 'a \n'],
  ['ws-tail-ineligible', 'x\n '],
  ['ws-tail-space-run', 'a\n  '],
  ['block-seq-item', ['a\nb']],

  // ── blocks: folded (long lines) ─────────────────────────────────────────
  ['folded-basic', 'a\n' + words(20) + '\n'],
  ['folded-k', { k: 'a\n' + words(20) + '\n' }],
  ['folded-unbreakable', 'a\n' + 'x'.repeat(90) + '\nb\n'],
  ['folded-moreind', 'a\n  ind ' + words(20) + '\nb\n'],
  ['folded-start-space', ' a\n' + words(20) + '\n'],
  ['folded-blank-mid', 'a\n\n' + words(20) + '\n\nb\n'],

  // ── folding: plain / single / double ────────────────────────────────────
  ['plain-fold', words(30)],
  ['fold-after-long-word', 'verylongwordhere ' + 'y'.repeat(99)],
  ['double-chunk', 'zz: ' + 'x'.repeat(100)],
  ['double-chunk-k', { k: 'zz: ' + 'x'.repeat(100) }],
  ['double-chunk-seq', ['zz: ' + 'x'.repeat(100)]],
  ['esc-cut-twice', { k: 'zz: ' + 'y'.repeat(200) }],
  ['single-fold', 'a: "' + 'x'.repeat(100)],
  ['trail-space-fold', 'word '.repeat(15).trimEnd() + '  finalword'],
  ['fold-under-key', { k: words(30) }],
  ['fold-deep', { a: { b: { c: words(30) } } }],
  ['fold-long-key-map', { ['k'.repeat(60)]: words(30) }],
  ['double-mid-space', 'zz: ' + 'y'.repeat(70) + ' ' + 'z'.repeat(70)],

  // ── the shape that started the port ─────────────────────────────────────
  [
    'agent-config',
    {
      raw: {
        model: {
          model: 'qwen-3.8-27b',
          base_url: 'https://infer.outcroplabs.com/v1',
          provider: 'custom'
        },
        model_aliases: {},
        fallback_providers: []
      },
      main: { model: 'qwen-3.8-27b', endpoint: 'outcrop-labs' },
      aliases: [],
      fallbacks: []
    }
  ],
  [
    'agent-config-strings',
    {
      soul: 'You are a careful assistant.',
      quirks: ['yes', 'no', 'on', 'off'],
      ratio: '0.9e-3',
      mtime: '2026-08-29T10:00:00Z'
    }
  ]
]

const vector = CASES.map(([name, value]) => ({
  name,
  input: JSON.stringify(value),
  yaml: stringify(value)
}))

const json = JSON.stringify({ cases: vector }, null, 2) + '\n'
const check = process.argv.includes('--check')
if (check) {
  const committed = readFileSync(FIXTURE, 'utf8')
  if (committed !== json) {
    console.error('[gen-yaml-vectors] committed fixture is stale — rerun `bun run api:vectors:yaml`')
    process.exit(1)
  }
  console.log(`[gen-yaml-vectors] ${vector.length} cases up to date`)
} else {
  writeFileSync(FIXTURE, json)
  console.log(`[gen-yaml-vectors] wrote ${vector.length} cases to ${FIXTURE}`)
}
