// The host card's permission arithmetic, pinned: what is waiting is the App's
// ask minus what the installation accepted, level by level, exactly as the
// server's `missing_from` computes the record's `pending`; an unknown side is
// never "nothing waiting".
import { describe, expect, it } from 'vitest'
import { checkedAgo, installationDiff, pendingPermissions, permissionEntries, permissionLabel, permissionSummary, type PermissionMap } from './git-permissions'

const app: PermissionMap = { contents: 'write', metadata: 'read', pull_requests: 'write', workflows: 'write', checks: 'read' }

describe('pendingPermissions', () => {
  it('is each asked key the installation lacks, or holds below the asked level, at the asked level', () => {
    // It took pull_requests at read only, and never took workflows.
    const held: PermissionMap = { checks: 'read', contents: 'write', metadata: 'read', pull_requests: 'read' }
    expect(pendingPermissions(app, held)).toEqual({ pull_requests: 'write', workflows: 'write' })
  })

  it('is nothing when the installation holds the ask, or more', () => {
    expect(pendingPermissions(app, app)).toEqual({})
    expect(pendingPermissions({ contents: 'read' }, { contents: 'write' })).toEqual({})
  })

  it('asks for all of it from an installation that holds nothing', () => {
    expect(pendingPermissions({ contents: 'write' }, {})).toEqual({ contents: 'write' })
    expect(pendingPermissions(null, {})).toEqual({})
  })
})

describe('installationDiff', () => {
  it('lists the ask, the accepted set and what waits, each by key', () => {
    const diff = installationDiff(app, { contents: 'write', metadata: 'read', pull_requests: 'write', checks: 'read' })
    expect(diff.known).toBe(true)
    expect(diff.asked.map((e) => e.key)).toEqual(['checks', 'contents', 'metadata', 'pull_requests', 'workflows'])
    expect(diff.accepted.map((e) => e.key)).toEqual(['checks', 'contents', 'metadata', 'pull_requests'])
    expect(diff.waiting).toEqual([{ key: 'workflows', level: 'write' }])
  })

  it('says nothing waits when either side is unknown', () => {
    expect(installationDiff(app, null)).toEqual({ asked: permissionEntries(app), accepted: [], waiting: [], known: false })
    expect(installationDiff(null, { contents: 'write' }).waiting).toEqual([])
    expect(installationDiff(undefined, undefined).known).toBe(false)
  })
})

describe('labels', () => {
  it('spells a key the way an admin reads it', () => {
    expect(permissionLabel({ key: 'pull_requests', level: 'write' })).toBe('pull requests: write')
    expect(permissionSummary({ metadata: 'read', contents: 'write' })).toBe('contents: write, metadata: read')
    expect(permissionSummary(null)).toBe('nothing')
  })

  it('says how long ago a check was, and never invents one', () => {
    const now = Date.parse('2026-11-16T12:00:00.000Z')
    expect(checkedAgo(null, now)).toBe('never checked')
    expect(checkedAgo('not a date', now)).toBe('never checked')
    expect(checkedAgo('2026-11-16T11:59:50.000Z', now)).toBe('checked just now')
    expect(checkedAgo('2026-11-16T11:56:00.000Z', now)).toBe('checked 4 min ago')
    expect(checkedAgo('2026-11-16T09:00:00.000Z', now)).toBe('checked 3 h ago')
    expect(checkedAgo('2026-11-12T12:00:00.000Z', now)).toBe('checked 4 days ago')
  })
})
