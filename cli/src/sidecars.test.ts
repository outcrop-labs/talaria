// The shared sidecar plane's contract — the one cluster with no gate before
// this file. Three things have to hold together, and each is a silent failure
// without the others:
//
//   1. ORDER. docker/sidecars.compose.yml is layered IN FRONT of every stack
//      file (compose merges by service name, the later file winning), so a
//      stack's own container_name/ports/networks and a devbox's `!reset` only
//      win from there. stackComposeFiles is the single spelling of that.
//   2. ONE HOME. The six sidecars' images, healthchecks and data volumes are
//      defined in the fragment and NOWHERE else: a stack file that grew one
//      back would drift from the other two on the next change.
//   3. WHAT A BOX DOES NOT RUN. embeddings and searxng are the primary dev
//      stack's, shared by every box (docker/devbox.compose.yml resets them
//      out) — if that reset is lost, every box pulls a second model and runs a
//      SearXNG with none of its settings.

import { describe, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { boxComposeSpec } from './cmd/box/shared'
import { COMPOSE_BASE, composeArgs, composeFileArgs, SIDECARS_COMPOSE, stackComposeFiles } from './compose'
import { fakeCtx } from './testing'

const ROOT = join(import.meta.dir, '..', '..')
const SIDECARS = ['postgres', 'redis', 'qdrant', 'embeddings', 'minio', 'searxng']
const STACKS = ['docker/compose.yml', 'docker/dev-compose.yml', 'docker/devbox.compose.yml']
const FRAGMENT = join(ROOT, SIDECARS_COMPOSE)

/** A compose file's services as raw block text, keyed by service name (an
 *  inline value like `!reset null` rides on the key's own line). Hand-rolled
 *  on purpose: this is a census of the files AS the repo writes them (two
 *  spaces under services, four under a key) and the package carries no YAML
 *  parser — a parser would also happily accept the shapes we are forbidding. */
function serviceBlocks(file: string): Record<string, string> {
  const blocks: Record<string, string> = {}
  let current = ''
  let inServices = false
  for (const line of readFileSync(file, 'utf8').split('\n')) {
    if (/^services:\s*$/.test(line)) {
      inServices = true
      continue
    }
    if (!inServices) continue
    if (/^[a-z][a-z0-9-]*:/.test(line)) break // the next top-level key
    if (line.startsWith('#')) continue
    const key = /^ {2}([a-z][a-z0-9-]*):(.*)$/.exec(line)
    if (key) {
      current = key[1]
      blocks[current] = key[2].trim()
      continue
    }
    if (current) blocks[current] += `\n${line}`
  }
  return blocks
}

/** The named services whose block in `file` matches `shape` — empty is the
 *  pass, and a failure names the offenders. */
function offenders(file: string, names: string[], shape: RegExp): string[] {
  const blocks = serviceBlocks(file)
  return names.filter((name) => shape.test(blocks[name] ?? ''))
}

describe('sidecar plane — merge order', () => {
  test('the fragment is FIRST in a stack list, the stack itself after', () => {
    expect(stackComposeFiles('/r', 'docker/dev-compose.yml')).toEqual([
      join('/r', SIDECARS_COMPOSE),
      join('/r', 'docker/dev-compose.yml'),
    ])
  })

  test('deploy/service get the same pair, and COMPOSE_FILE replaces BOTH', () => {
    expect(composeFileArgs(null, COMPOSE_BASE)).toEqual(['-f', SIDECARS_COMPOSE, '-f', COMPOSE_BASE])
    // An explicit -f beats the env, so honoring COMPOSE_FILE means passing
    // neither — the operator's list has to name the fragment itself.
    expect(composeFileArgs('docker/compose.yml:docker/compose.registry.yml', COMPOSE_BASE)).toEqual([])
  })

  test("a box's argv layers the fragment before its own template", () => {
    // The slice of the real argv docker receives: -f fragment -f template …
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-sidecars-'))
    expect(composeArgs(boxComposeSpec(ctx, 'demo')).slice(0, 4)).toEqual([
      '-f',
      join(ctx.root, SIDECARS_COMPOSE),
      '-f',
      join(ctx.root, 'docker/devbox.compose.yml'),
    ])
  })
})

describe('sidecar plane — one definition, in the fragment', () => {
  test('the fragment declares exactly the six sidecars, fully specified', () => {
    const blocks = serviceBlocks(FRAGMENT)
    expect(Object.keys(blocks).sort()).toEqual([...SIDECARS].sort())
    expect(SIDECARS.filter((n) => !/^ {4}image: /m.test(blocks[n]!))).toEqual([])
    expect(SIDECARS.filter((n) => !/^ {4}restart: unless-stopped$/m.test(blocks[n]!))).toEqual([])
    expect(SIDECARS.filter((n) => !/^ {4}healthcheck:$/m.test(blocks[n]!))).toEqual([])
  })

  test('the fragment decides no ports, no networks, no container names', () => {
    // devbox publishes nothing and prod owns the `internal` network — a port
    // or a network in the shared definition would land on all three stacks.
    expect(offenders(FRAGMENT, SIDECARS, /^ {4}(ports|networks|container_name):/m)).toEqual([])
  })

  test('no stack redefines what the fragment owns', () => {
    for (const stack of STACKS) {
      const file = join(ROOT, stack)
      expect(offenders(file, SIDECARS, /^ {4}(image|restart|healthcheck|command):/m)).toEqual([])
      // The data volumes too — except searxng's, which each stack mounts
      // differently (prod a rendered named volume, dev a rendered bind).
      expect(offenders(file, SIDECARS.filter((n) => n !== 'searxng'), /^ {4}volumes:/m)).toEqual([])
    }
  })

  test('every stack keeps the four sidecars all three run', () => {
    for (const stack of STACKS) {
      const names = Object.keys(serviceBlocks(join(ROOT, stack)))
      expect([stack, ...['postgres', 'redis', 'qdrant', 'minio'].filter((n) => !names.includes(n))]).toEqual([stack])
    }
  })

  test('a box resets out the two services the primary stack hosts', () => {
    const devbox = readFileSync(join(ROOT, 'docker/devbox.compose.yml'), 'utf8')
    const blocks = serviceBlocks(join(ROOT, 'docker/devbox.compose.yml'))
    expect(blocks.embeddings?.trim()).toBe('!reset null')
    expect(blocks.searxng?.trim()).toBe('!reset null')
    // …and the fragment's TEI cache volume goes with the service.
    const volumes = /^volumes:\n([\s\S]*)$/m.exec(devbox)![1]!.split('\n').filter((l) => /^ {2}tei-cache:/.test(l))
    expect(volumes).toEqual(['  tei-cache: !reset null'])
  })
})