// Workbench mounts render verbatim into the fleet's compose volumes and the
// sandbox runs as root, so a mount string is an arbitrary host-filesystem
// grant. The audit's finding (S2) was that `agents.manage` — grantable to
// non-admins — could set one with no content validation at all, making
// `/var/run/docker.sock:/x` a path to host root.
//
// `mountError` is the default-deny half of that fix (the route's admin gate is
// the other). These tests pin the denials that matter and, just as important,
// the ordinary mounts that must keep working.
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mountError } from './workbench'

const ROOTS = '/srv/talaria/fleet,/srv/talaria/work'

beforeEach(() => {
  process.env.TALARIA_WORKBENCH_MOUNT_ROOTS = ROOTS
})
afterEach(() => {
  delete process.env.TALARIA_WORKBENCH_MOUNT_ROOTS
})

describe('mountError — the escalations it must refuse', () => {
  it('refuses the docker socket, which is host root by another name', () => {
    expect(mountError('/var/run/docker.sock:/var/run/docker.sock')).toMatch(/hand the container the host/)
  })

  it('refuses the socket’s parent directory, which brings the socket with it', () => {
    expect(mountError('/var/run:/hostrun')).toMatch(/hand the container the host/)
    expect(mountError('/run:/hostrun')).toMatch(/hand the container the host/)
  })

  it('refuses /proc, /sys and /dev', () => {
    for (const p of ['/proc', '/sys', '/dev']) {
      expect(mountError(`${p}:/host${p}`)).toMatch(/hand the container the host/)
    }
  })

  it('refuses the host root', () => {
    expect(mountError('/:/host')).toMatch(/host root is never mountable/)
  })

  it('refuses a path outside the allowed roots', () => {
    expect(mountError('/etc:/etc-ro:ro')).toMatch(/outside the allowed mount roots/)
    expect(mountError('/home/jon/.ssh:/keys')).toMatch(/outside the allowed mount roots/)
  })

  it('refuses traversal that climbs back out of an allowed root', () => {
    // resolve() collapses the ..; the check runs on the resolved path.
    expect(mountError('/srv/talaria/fleet/../../../etc:/etc')).toMatch(/outside the allowed mount roots/)
  })

  it('refuses traversal that lands on a denied source', () => {
    expect(mountError('/srv/talaria/fleet/../../../var/run/docker.sock:/sock')).toMatch(
      /hand the container the host/,
    )
  })

  it('refuses a destination that is not an absolute container path', () => {
    expect(mountError('/srv/talaria/fleet:relative')).toMatch(/destination must be an absolute path/)
    expect(mountError('/srv/talaria/fleet:/')).toMatch(/destination must be an absolute path/)
  })

  it('refuses a malformed mount string', () => {
    expect(mountError('/srv/talaria/fleet')).toMatch(/expected "source:\/dest/)
    expect(mountError('a:b:c:d')).toMatch(/expected "source:\/dest/)
  })

  it('refuses an unknown mode', () => {
    expect(mountError('/srv/talaria/fleet:/work:rwx')).toMatch(/unknown mode/)
  })

  it('refuses an empty source', () => {
    expect(mountError(':/work')).toMatch(/source required/)
  })

  it('refuses a relative host path masquerading as a named volume', () => {
    expect(mountError('../../etc:/etc')).toMatch(/absolute host path or a named volume/)
    expect(mountError('./secrets:/s')).toMatch(/absolute host path or a named volume/)
  })
})

describe('mountError — what must keep working', () => {
  it('accepts a path under an allowed root', () => {
    expect(mountError('/srv/talaria/fleet:/work')).toBeNull()
    expect(mountError('/srv/talaria/work/repos:/repos:ro')).toBeNull()
  })

  it('accepts an allowed root exactly', () => {
    expect(mountError('/srv/talaria/work:/w')).toBeNull()
  })

  it('accepts every mode compose actually uses', () => {
    for (const mode of ['ro', 'rw', 'z', 'Z', 'ro,z', 'rw,z', 'ro,Z', 'rw,Z']) {
      expect(mountError(`/srv/talaria/fleet:/work:${mode}`)).toBeNull()
    }
  })

  it('accepts named volumes, which never touch the host filesystem', () => {
    expect(mountError('agent-state:/state')).toBeNull()
    expect(mountError('talaria_cache.v2:/cache:rw')).toBeNull()
  })

  it('honours a widened root list', () => {
    expect(mountError('/srv/talaria/work/x:/x')).toBeNull()
    process.env.TALARIA_WORKBENCH_MOUNT_ROOTS = '/opt/data'
    expect(mountError('/srv/talaria/work/x:/x')).toMatch(/outside the allowed mount roots/)
    expect(mountError('/opt/data/sets:/sets')).toBeNull()
  })

  it('does not let a widened root re-open a denied source', () => {
    process.env.TALARIA_WORKBENCH_MOUNT_ROOTS = '/'
    expect(mountError('/var/run/docker.sock:/sock')).toMatch(/hand the container the host/)
  })
})
