// scripts/rustc-wrapper.sh resolves the REAL sccache, never mise's shim, and
// falls back to plain rustc. Run by `bun run check` — stdlib only, and
// hermetic: every binary the wrapper can reach is a fake in a temp dir, so the
// answer is the same on a laptop with mise, a CI runner without it, and a box.
//
// THE INCIDENT (follow-up A2, 2026-10-27). In mise's shims mode the `sccache`
// on PATH is the shim, which reads the CURRENT directory's mise config. Cargo
// runs rustc for a registry crate from ~/.cargo/registry/src/<crate>, where no
// config pins sccache, so the shim said "mise ERROR No version is set for shim:
// sccache" and the wrapper's `exec` turned that into a failed build. Every
// case below runs from such a directory — outside the repo — on purpose.

import { spawnSync } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const WRAPPER = join(ROOT, 'scripts/rustc-wrapper.sh')

let failed = 0
let ran = 0

function check(name, fn) {
  ran++
  try {
    fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.message : e}`)
  }
}

function eq(actual, expected, what) {
  if (actual !== expected) throw new Error(`${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`)
}

function exe(path, body) {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, `#!/bin/sh\n${body}\n`)
  chmodSync(path, 0o755)
}

/** The real path of a utility the wrapper shells out to, so the fixture PATH
 *  can carry it without carrying the rest of /usr/bin (which may hold a real
 *  sccache or mise of its own and make a case pass for the wrong reason). */
function tool(name) {
  const r = spawnSync('/bin/sh', ['-c', `command -v ${name}`], { encoding: 'utf8' })
  const p = r.stdout.trim()
  if (r.status !== 0 || !p) throw new Error(`no ${name} on this host's PATH`)
  return p
}

const base = realpathSync(mkdtempSync(join(tmpdir(), 'rustc-wrapper-test-')))
try {
  const tools = join(base, 'tools')
  mkdirSync(tools)
  for (const t of ['dirname', 'readlink']) symlinkSync(tool(t), join(tools, t))

  // A registry crate dir: outside the repo, no mise config anywhere above it.
  const registry = join(base, 'cargo/registry/src/index.crates.io-x/adler2-2.0.1')
  mkdirSync(registry, { recursive: true })

  const rustc = join(base, 'toolchain/rustc')
  exe(rustc, 'echo "rustc-ran $*"')
  const realSccache = join(base, 'installs/sccache/0.18/sccache')
  exe(realSccache, 'echo "sccache-ran $*"')

  // `mise` itself: as `mise which sccache` it answers $FAKE_MISE_WHICH (or
  // fails when that is empty) and records the directory it was asked from.
  const miseBin = join(base, 'local/bin/mise')
  const miseCwd = join(base, 'mise-cwd')
  exe(
    miseBin,
    [
      'case "$(basename "$0")" in',
      '  mise)',
      `    pwd > '${miseCwd}'`,
      '    [ "$1" = which ] && [ "$2" = sccache ] && [ -n "$FAKE_MISE_WHICH" ] && { echo "$FAKE_MISE_WHICH"; exit 0; }',
      '    exit 1 ;;',
      '  *)',
      // What the real shim prints from a directory with no version pinned.
      '    echo "mise ERROR No version is set for shim: $(basename "$0")" >&2; exit 1 ;;',
      'esac',
    ].join('\n'),
  )
  symlinkSync(tool('basename'), join(tools, 'basename'))
  const shims = join(base, 'share/mise/shims')
  mkdirSync(shims, { recursive: true })
  symlinkSync(miseBin, join(shims, 'sccache'))
  // A shim directory mise was told to put somewhere else: still a symlink to
  // the mise binary, and still not the binary.
  const customShims = join(base, 'custom-shims')
  mkdirSync(customShims)
  symlinkSync(miseBin, join(customShims, 'sccache'))
  const realOnPath = join(base, 'real-bin')
  mkdirSync(realOnPath)
  symlinkSync(realSccache, join(realOnPath, 'sccache'))

  const run = (pathDirs, env = {}) => {
    rmSync(miseCwd, { force: true })
    const r = spawnSync('/bin/sh', [WRAPPER, rustc, '-vV'], {
      cwd: registry,
      encoding: 'utf8',
      env: { HOME: base, PATH: [...pathDirs, tools].join(':'), ...env },
    })
    return {
      status: r.status,
      out: r.stdout.trim(),
      err: r.stderr.trim(),
      askedFrom: existsSync(miseCwd) ? readFileSync(miseCwd, 'utf8').trim() : null,
    }
  }

  check('shims-only PATH: the real binary comes from `mise which`, asked from the repo root', () => {
    const r = run([shims, dirname(miseBin)], { FAKE_MISE_WHICH: realSccache })
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `sccache-ran ${rustc} -vV`, 'output')
    eq(r.askedFrom, realpathSync(ROOT), 'mise was asked from')
  })

  check('shims-only PATH and mise cannot answer: plain rustc, not a failed build', () => {
    const r = run([shims, dirname(miseBin)], { FAKE_MISE_WHICH: '' })
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `rustc-ran -vV`, 'output')
  })

  check('shims-only PATH and no mise binary at all: plain rustc', () => {
    const r = run([shims])
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `rustc-ran -vV`, 'output')
  })

  check('a real sccache on PATH is used directly, ahead of the shim, without asking mise', () => {
    const r = run([realOnPath, shims, dirname(miseBin)], { FAKE_MISE_WHICH: '/nowhere' })
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `sccache-ran ${rustc} -vV`, 'output')
    eq(r.askedFrom, null, 'mise asked')
  })

  check('a real sccache AFTER the shims dir still wins: the shim is skipped, not exec-ed', () => {
    const r = run([shims, realOnPath, dirname(miseBin)])
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `sccache-ran ${rustc} -vV`, 'output')
  })

  check('a shim in a custom shims dir is recognised by what it links to', () => {
    const r = run([customShims, dirname(miseBin)], { FAKE_MISE_WHICH: '' })
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `rustc-ran -vV`, 'output')
  })

  check('`mise which` naming a shim is not trusted either', () => {
    const r = run([dirname(miseBin)], { FAKE_MISE_WHICH: join(shims, 'sccache') })
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `rustc-ran -vV`, 'output')
  })

  check('no sccache anywhere: plain rustc (the original promise)', () => {
    const r = run([])
    eq(r.status, 0, `exit (stderr: ${r.err})`)
    eq(r.out, `rustc-ran -vV`, 'output')
  })
} finally {
  rmSync(base, { recursive: true, force: true })
}

if (failed) {
  console.error(`rustc-wrapper: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`rustc-wrapper: ${ran} checks, all clean`)
