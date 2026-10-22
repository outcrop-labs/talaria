// The COMPOSE_FILE export repair: the exact line, in every shape a profile
// spells it, and nothing else in the file. The integration half — what
// composeFileEnv says about it — drives a fake ctx whose HOME is a tmp dir.

import { describe, expect, test } from 'bun:test'
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { profileFiles, repairComposeFileExport, tildePath } from './profile-export'
import { composeFileEnv, SIDECARS_COMPOSE } from './compose'
import { fakeCtx } from './testing'

// The value on the customer VM, 2026-10-22.
const BROKEN = 'docker/compose.yml:docker/compose.registry.yml:docker/compose.vm.yml'
const FIXED = `${SIDECARS_COMPOSE}:${BROKEN}`

const home = () => mkdtempSync(join(tmpdir(), 'talaria-home-'))

describe('repairComposeFileExport', () => {
  test('rewrites the export line and nothing else', () => {
    const h = home()
    const rc = join(h, '.bashrc')
    const before = `# my bashrc\nalias ll='ls -l'\nexport COMPOSE_FILE=${BROKEN}\nexport EDITOR=vim\n`
    writeFileSync(rc, before)
    const out = repairComposeFileExport([rc], BROKEN, FIXED)
    expect(out).toEqual([{ file: rc, status: 'fixed' }])
    expect(readFileSync(rc, 'utf8')).toBe(before.replace(`COMPOSE_FILE=${BROKEN}`, `COMPOSE_FILE=${FIXED}`))
  })

  test('keeps quotes, a trailing comment, and the bare /etc/environment shape', () => {
    const h = home()
    const a = join(h, 'a')
    const b = join(h, 'b')
    const c = join(h, 'c')
    writeFileSync(a, `  export COMPOSE_FILE="${BROKEN}"   # registry mode\n`)
    writeFileSync(b, `COMPOSE_FILE='${BROKEN}'\n`)
    writeFileSync(c, `set -gx COMPOSE_FILE ${BROKEN}\n`)
    const out = repairComposeFileExport([a, b, c], BROKEN, FIXED)
    expect(out.map((r) => r.status)).toEqual(['fixed', 'fixed', 'fixed'])
    expect(readFileSync(a, 'utf8')).toBe(`  export COMPOSE_FILE="${FIXED}"   # registry mode\n`)
    expect(readFileSync(b, 'utf8')).toBe(`COMPOSE_FILE='${FIXED}'\n`)
    expect(readFileSync(c, 'utf8')).toBe(`set -gx COMPOSE_FILE ${FIXED}\n`)
  })

  test('leaves an operator value it was not asked about alone', () => {
    const h = home()
    const rc = join(h, '.profile')
    const text = `export COMPOSE_FILE=docker/compose.yml:docker/compose.other.yml\nexport COMPOSE_FILE_EXTRA=${BROKEN}\nexport COMPOSE_FILE=$HOME/x.yml\n`
    writeFileSync(rc, text)
    expect(repairComposeFileExport([rc], BROKEN, FIXED)).toEqual([])
    expect(readFileSync(rc, 'utf8')).toBe(text)
  })

  test('a file fixed earlier is reported as already, not rewritten', () => {
    const h = home()
    const rc = join(h, '.bashrc')
    writeFileSync(rc, `export COMPOSE_FILE=${FIXED}\n`)
    expect(repairComposeFileExport([rc], BROKEN, FIXED)).toEqual([{ file: rc, status: 'already' }])
  })

  test('missing files are skipped', () => {
    expect(repairComposeFileExport([join(home(), 'nope')], BROKEN, FIXED)).toEqual([])
  })

  test.skipIf(process.getuid?.() === 0)('an export this user cannot write is reported, not thrown', () => {
    const h = home()
    const rc = join(h, '.profile')
    writeFileSync(rc, `export COMPOSE_FILE=${BROKEN}\n`)
    chmodSync(rc, 0o444)
    expect(repairComposeFileExport([rc], BROKEN, FIXED)).toEqual([{ file: rc, status: 'readonly' }])
    expect(readFileSync(rc, 'utf8')).toBe(`export COMPOSE_FILE=${BROKEN}\n`)
  })
})

describe('profileFiles', () => {
  test('nothing without HOME — a test ctx never reaches a real home', () => {
    expect(profileFiles({})).toEqual([])
  })

  test("the user's own files first, then the system's", () => {
    const files = profileFiles({ HOME: '/home/op' })
    expect(files[0]).toBe('/home/op/.profile')
    expect(files).toContain('/home/op/.bashrc')
    expect(files).toContain('/home/op/.config/fish/config.fish')
    expect(files.indexOf('/home/op/.zshrc')).toBeLessThan(files.indexOf('/etc/environment'))
  })

  test('tildePath shortens the home prefix only', () => {
    expect(tildePath('/home/op/.bashrc', '/home/op')).toBe('~/.bashrc')
    expect(tildePath('/etc/environment', '/home/op')).toBe('/etc/environment')
    expect(tildePath('/home/operator/.bashrc', '/home/op')).toBe('/home/operator/.bashrc')
  })
})

describe('composeFileEnv — the export is fixed where it lives', () => {
  test('fixes ~/.bashrc, says so once, and does not warn', () => {
    const h = home()
    writeFileSync(join(h, '.bashrc'), `export COMPOSE_FILE=${BROKEN}\n`)
    const ctx = fakeCtx({ env: { HOME: h, COMPOSE_FILE: BROKEN } })
    expect(composeFileEnv(ctx)).toBe(FIXED)
    expect(composeFileEnv(ctx)).toBe(FIXED)
    expect(ctx.env.COMPOSE_FILE).toBe(FIXED)
    expect(readFileSync(join(h, '.bashrc'), 'utf8')).toBe(`export COMPOSE_FILE=${FIXED}\n`)
    expect(ctx.logLines.filter((l) => l.kind === 'ok')).toHaveLength(1)
    expect(ctx.logLines.find((l) => l.kind === 'ok')!.msg).toContain('~/.bashrc')
    expect(ctx.logLines.some((l) => l.kind === 'warn')).toBe(false)
  })

  test('a shell that still holds the old value gets one quiet line, not the warning', () => {
    const h = home()
    writeFileSync(join(h, '.bashrc'), `export COMPOSE_FILE=${FIXED}\n`)
    const ctx = fakeCtx({ env: { HOME: h, COMPOSE_FILE: BROKEN } })
    expect(composeFileEnv(ctx)).toBe(FIXED)
    expect(ctx.logLines.some((l) => l.kind === 'warn')).toBe(false)
    const skip = ctx.logLines.find((l) => l.kind === 'skip')
    expect(skip?.msg).toContain('source ~/.bashrc')
  })

  test('an export it cannot find still warns, as before', () => {
    const h = home()
    mkdirSync(join(h, '.config'), { recursive: true })
    const ctx = fakeCtx({ env: { HOME: h, COMPOSE_FILE: BROKEN } })
    expect(composeFileEnv(ctx)).toBe(FIXED)
    expect(ctx.logLines.find((l) => l.kind === 'warn')?.msg).toContain('omits docker/sidecars.compose.yml')
  })

  test('a list that already carries the plane is untouched and silent', () => {
    const h = home()
    writeFileSync(join(h, '.bashrc'), `export COMPOSE_FILE=${FIXED}\n`)
    const ctx = fakeCtx({ env: { HOME: h, COMPOSE_FILE: FIXED } })
    expect(composeFileEnv(ctx)).toBe(FIXED)
    expect(ctx.logLines).toHaveLength(0)
  })
})
