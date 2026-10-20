import assert from 'node:assert/strict'
import { describe, test } from 'vitest'
import type { Blueprint } from '@/lib/blueprints'
import type { SkillOwner } from '@/lib/skills'
import {
  blueprintBodyFrom,
  commandItems,
  expandSlashCommand,
  filterCommands,
  isBareCommand,
  offersSave,
  platformSkills,
  saveCommand,
  type CommandSkill,
} from './command-suggest'

// The "/" menu's rules. Two of them carry the whole feature: which rows a
// query offers, and what the agent is actually sent when a command goes out.

const skill = (name: string, description = `the ${name} skill`): CommandSkill => ({ name, description })

const blueprint = (over: Partial<Blueprint> & { id: string; name: string }): Blueprint => ({
  body: 'A body',
  shared: false,
  ownerUserId: 'u1',
  surface: 'work',
  sort: 0,
  createdAt: '2026-10-16T00:00:00Z',
  updatedAt: '2026-10-16T00:00:00Z',
  ...over,
})

describe('platformSkills — the shipped set out of the library read', () => {
  const owners: SkillOwner[] = [
    { owner: 'shared', label: 'Shared', skills: [{ name: 'github', description: 'Git hosting', platform: true }, { name: 'house-style', description: 'Ours', platform: false }] },
    { owner: 'ada', label: 'Ada', skills: [{ name: 'github', description: 'Git hosting', platform: true }, { name: 'browser', description: 'Browse', platform: true }] },
  ]

  test('only platform skills, de-duplicated across owners, by name', () => {
    assert.deepEqual(platformSkills(owners).map((s) => s.name), ['browser', 'github'])
  })

  test('an org-authored skill is not a command', () => {
    assert.equal(platformSkills(owners).some((s) => s.name === 'house-style'), false)
  })

  test('an owner with no skills array is not a crash', () => {
    const odd = [{ owner: 'x', label: 'X' } as unknown as SkillOwner]
    assert.deepEqual(platformSkills(odd), [])
  })
})

describe('filterCommands — what the menu offers', () => {
  const skills = [skill('browser'), skill('github'), skill('research')]
  const blueprints = [
    blueprint({ id: 'b1', name: 'bug report', body: '# Bug\nSteps to reproduce' }),
    blueprint({ id: 'b2', name: 'github handoff', body: 'Hand this to the next agent' }),
  ]

  test('an empty query opens the whole menu, skills first', () => {
    const rows = filterCommands('', skills, blueprints)
    assert.deepEqual(rows.map((r) => r.label), ['browser', 'github', 'research', 'bug report', 'github handoff'])
    assert.deepEqual(rows.map((r) => r.kind), ['skill', 'skill', 'skill', 'blueprint', 'blueprint'])
  })

  test('a prefix match outranks a substring match', () => {
    const rows = filterCommands('b', [skill('github'), skill('browser')], [])
    assert.deepEqual(rows.map((r) => r.label), ['browser', 'github'])
  })

  test('a skill outranks a blueprint at the same rank', () => {
    const rows = filterCommands('git', skills, blueprints)
    assert.deepEqual(rows.map((r) => r.label), ['github', 'github handoff'])
  })

  test('non-matches are dropped and the match is case-insensitive', () => {
    assert.deepEqual(filterCommands('GITH', skills, blueprints).map((r) => r.label), ['github', 'github handoff'])
    assert.deepEqual(filterCommands('zzz', skills, blueprints), [])
  })

  test('at most eight matches — the save row is appended after the cap', () => {
    const many = Array.from({ length: 12 }, (_, i) => skill(`s${i}`))
    assert.equal(filterCommands('s', many, []).length, 8)
  })

  test("a blueprint row carries its body and its first line, and the viewer's own offers a delete", () => {
    const rows = filterCommands('bug', [], blueprints, { viewerId: 'u1' })
    assert.equal(rows[0]?.body, '# Bug\nSteps to reproduce')
    assert.equal(rows[0]?.detail, 'Bug')
    assert.equal(rows[0]?.canDelete, true)
  })

  test("someone else's row, and a shared one, offer no delete", () => {
    const shared = blueprint({ id: 'b3', name: 'shared brief', shared: true, ownerUserId: null })
    const rows = filterCommands('', [], [shared, blueprint({ id: 'b4', name: 'theirs', ownerUserId: 'u9' })], { viewerId: 'u1' })
    assert.deepEqual(rows.map((r) => r.canDelete), [false, false])
  })
})

describe('the save row — a draft becomes a blueprint', () => {
  test('a bare command is the command being typed, not a draft', () => {
    for (const text of ['/', '/gi', '/github']) {
      assert.equal(isBareCommand(text), true, text)
      assert.equal(offersSave(text), false, text)
      assert.equal(saveCommand(text), null, text)
    }
  })

  test('the body drops the command-in-progress off the front', () => {
    assert.equal(blueprintBodyFrom('/ Draft the weekly brief'), 'Draft the weekly brief')
    assert.equal(blueprintBodyFrom('/gi Draft the weekly brief'), 'Draft the weekly brief')
    // The menu opened on a NEW paragraph after the draft: the trailing token
    // is the gesture, not the text.
    assert.equal(blueprintBodyFrom('para1\n\npara2\n\n/'), 'para1\n\npara2')
    assert.equal(blueprintBodyFrom('para1\n\n/gi'), 'para1')
    assert.equal(offersSave('para1\n\n/gi'), true)
    assert.equal(offersSave('/gi'), false)
    assert.equal(blueprintBodyFrom('Draft the weekly brief'), 'Draft the weekly brief')
  })

  test('a draft behind the slash is savable, and carries its body', () => {
    const row = saveCommand('/ Draft the weekly brief')
    assert.equal(row?.kind, 'save')
    assert.equal(row?.body, 'Draft the weekly brief')
    assert.equal(row?.label, 'Save as blueprint')
  })

  test('an empty well offers nothing to save', () => {
    assert.equal(offersSave(''), false)
    assert.equal(offersSave('   '), false)
  })

  test('commandItems puts the save row last, and only when there is one', () => {
    const args = { query: '', skills: [skill('github')], blueprints: [], viewerId: 'u1' }
    assert.deepEqual(commandItems({ ...args, text: '/' }).map((r) => r.kind), ['skill'])
    assert.deepEqual(commandItems({ ...args, text: '/ write the brief' }).map((r) => r.kind), ['skill', 'save'])
  })
})

describe('expandSlashCommand — what the agent is sent', () => {
  const known = ['github', 'browser']

  test('the command becomes the sentence, and the ask survives verbatim', () => {
    assert.equal(expandSlashCommand('/github fix the CI', known), 'Use your `github` skill. fix the CI')
  })

  test('a command on its own is still the request to use the skill', () => {
    assert.equal(expandSlashCommand('/github', known), 'Use your `github` skill.')
  })

  test('the library name is what gets quoted, whatever case was typed', () => {
    assert.equal(expandSlashCommand('/GitHub fix the CI', known), 'Use your `github` skill. fix the CI')
  })

  test('an unknown command is the person typing a slash — untouched', () => {
    assert.equal(expandSlashCommand('/deploy the thing', known), '/deploy the thing')
    assert.equal(expandSlashCommand('/github fix the CI', []), '/github fix the CI')
  })

  test('a slash that is not a leading command is never a command', () => {
    for (const text of [
      'see https://github.com/x/y for the build',
      'the 2/3 of it that works',
      'run ./github fix',
      '/github/workflows is the folder',
      'ask /github about it',
      '/',
      '',
    ]) {
      assert.equal(expandSlashCommand(text, known), text, text)
    }
  })

  test('a newline after the command stays a newline', () => {
    assert.equal(expandSlashCommand('/github\nfix the CI', known), 'Use your `github` skill.\nfix the CI')
  })

  test('the expansion is idempotent — an already-sent sentence is not a command', () => {
    const once = expandSlashCommand('/github fix the CI', known)
    assert.equal(expandSlashCommand(once, known), once)
  })
})

describe('expandSlashCommand — later lines', () => {
  test('a command on a later line is expanded where it stands', () => {
    assert.equal(
      expandSlashCommand('look at this\n\n/github fix the CI', ['github']),
      'look at this\n\nUse your `github` skill. fix the CI',
    )
    assert.equal(expandSlashCommand('see https://x.y/github now', ['github']), 'see https://x.y/github now')
  })
})
