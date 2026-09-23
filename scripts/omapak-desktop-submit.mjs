#!/usr/bin/env node
// Refresh apps/app.talaria.desktop in an omapak checkout from a Talaria tag.
//
// Omapak builds the desktop shell from a pinned git tag plus vendored
// cargo/bun sources (the build is offline). A published desktop release
// has to move that pin, or the store keeps shipping the previous tag.
// The workflow opens the PR; this script only rewrites the submission.
//
// stdlib only — scripts/ has no install, and `bun run check` runs anywhere.

import { readFileSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const APP = 'app.talaria.desktop'

export function cargoSources(lockText) {
  const sources = []
  for (const block of lockText.split('\n[[package]]\n').slice(1)) {
    const name = field(block, 'name')
    const version = field(block, 'version')
    const source = field(block, 'source')
    const checksum = field(block, 'checksum')
    if (!name || !version || !checksum) continue
    if (!source?.startsWith('registry+')) continue
    const dest = `cargo/vendor/${name}-${version}`
    sources.push({
      type: 'archive',
      'archive-type': 'tar-gzip',
      url: `https://static.crates.io/crates/${name}/${name}-${version}.crate`,
      sha256: checksum,
      dest,
    })
    sources.push({
      type: 'inline',
      contents: JSON.stringify({ package: checksum, files: {} }),
      dest,
      'dest-filename': '.cargo-checksum.json',
    })
  }
  sources.sort((a, b) => a.dest.localeCompare(b.dest) || a.type.localeCompare(b.type))
  return sources
}

export function nodeSources(lockText) {
  const lock = JSON.parse(lockText.replace(/,(\s*[}\]])/g, '$1'))
  const sources = []
  for (const entry of Object.values(lock.packages ?? {})) {
    if (!Array.isArray(entry)) continue
    const ident = entry[0]
    const at = ident.lastIndexOf('@')
    if (at <= 0) continue
    const name = ident.slice(0, at)
    const version = ident.slice(at + 1)
    const meta = entry.find((part) => part && typeof part === 'object' && !Array.isArray(part))
    if (!linuxX64(meta)) continue
    const integrity = entry.find((part) => typeof part === 'string' && part.startsWith('sha512-'))
    if (!integrity) continue
    const bare = name.startsWith('@') ? name.slice(name.indexOf('/') + 1) : name
    sources.push({
      type: 'archive',
      url: `https://registry.npmjs.org/${name}/-/${bare}-${version}.tgz`,
      sha512: Buffer.from(integrity.slice('sha512-'.length), 'base64').toString('hex'),
      dest: `bun_cache/${name}@${version}@@@1`,
    })
  }
  sources.sort((a, b) => a.dest.localeCompare(b.dest))
  return sources
}

function linuxX64(meta) {
  if (!meta) return true
  const os = meta.os
  const cpu = meta.cpu
  if (os && !includes(os, 'linux')) return false
  if (cpu && !includes(cpu, 'x64')) return false
  return true
}

function includes(value, want) {
  return Array.isArray(value) ? value.includes(want) : value === want
}

function field(block, key) {
  const match = block.match(new RegExp(`^${key} = "([^"]*)"`, 'm'))
  return match?.[1]
}

function rustChannel(toolchainToml) {
  const match = toolchainToml.match(/^channel = "([^"]+)"/m)
  if (!match) throw new Error('rust-toolchain.toml has no channel')
  return match[1]
}

function bunPin(actionYml) {
  const match = actionYml.match(/bun:[\s\S]*?default: '([^']+)'/)
  if (!match) throw new Error('setup-runtime action has no bun default')
  return match[1]
}

async function sha256Url(url) {
  const sidecar = await fetch(`${url}.sha256`)
  if (sidecar.ok) {
    const text = await sidecar.text()
    const hash = text.trim().split(/\s+/)[0]
    if (/^[0-9a-f]{64}$/i.test(hash)) return hash.toLowerCase()
  }
  const body = await fetch(url)
  if (!body.ok) throw new Error(`fetching ${url}: ${body.status}`)
  return createHash('sha256').update(Buffer.from(await body.arrayBuffer())).digest('hex')
}

function replacePinned(yml, { tag, commit, rust, bun }) {
  let next = yml
  next = next.replace(/tag: \S+/, `tag: ${tag}`)
  next = next.replace(/commit: [0-9a-f]{40}/, `commit: ${commit}`)
  next = next.replace(
    /https:\/\/static\.rust-lang\.org\/dist\/rust-[^/]+-x86_64-unknown-linux-gnu\.tar\.xz/,
    `https://static.rust-lang.org/dist/rust-${rust.version}-x86_64-unknown-linux-gnu.tar.xz`,
  )
  next = next.replace(
    /https:\/\/github\.com\/oven-sh\/bun\/releases\/download\/bun-v[^/]+\/bun-linux-x64\.zip/,
    `https://github.com/oven-sh/bun/releases/download/bun-v${bun.version}/bun-linux-x64.zip`,
  )
  next = replaceShaAfter(next, 'static.rust-lang.org', rust.sha256)
  next = replaceShaAfter(next, 'oven-sh/bun', bun.sha256)
  return next
}

function replaceShaAfter(text, needle, sha) {
  const at = text.indexOf(needle)
  if (at < 0) throw new Error(`manifest has no ${needle} source`)
  return text.slice(0, at) + text.slice(at).replace(/sha256: [0-9a-f]{64}/, `sha256: ${sha}`)
}

function replaceMetadata(text, { tag, rust, bun }) {
  return text
    .replace(/at tag [^:\s]+/, `at tag ${tag}`)
    .replace(/Rust \d+\.\d+\.\d+/, `Rust ${rust}`)
    .replace(/bun \d+\.\d+\.\d+/, `bun ${bun}`)
}

export async function submit({ talaria, omapak, tag, commit, fetchShas }) {
  const rustVersion = rustChannel(readFileSync(join(talaria, 'desktop/src-tauri/rust-toolchain.toml'), 'utf8'))
  const bunVersion = bunPin(readFileSync(join(talaria, '.github/actions/setup-runtime/action.yml'), 'utf8'))
  const app = join(omapak, 'apps', APP)
  const ymlPath = join(app, `${APP}.yml`)
  const yml = readFileSync(ymlPath, 'utf8')
  const rustSha = fetchShas
    ? await sha256Url(`https://static.rust-lang.org/dist/rust-${rustVersion}-x86_64-unknown-linux-gnu.tar.xz`)
    : shaFrom(yml, 'static.rust-lang.org')
  const bunSha = fetchShas
    ? await sha256Url(`https://github.com/oven-sh/bun/releases/download/bun-v${bunVersion}/bun-linux-x64.zip`)
    : shaFrom(yml, 'oven-sh/bun')

  const cargo = cargoSources(readFileSync(join(talaria, 'desktop/src-tauri/Cargo.lock'), 'utf8'))
  const node = nodeSources(readFileSync(join(talaria, 'desktop/bun.lock'), 'utf8'))
  writeFileSync(join(app, 'cargo-sources.json'), `${JSON.stringify(cargo, null, 4)}\n`)
  writeFileSync(join(app, 'node-sources.json'), `${JSON.stringify(node, null, 1)}\n`)
  writeFileSync(
    ymlPath,
    replacePinned(yml, {
      tag,
      commit,
      rust: { version: rustVersion, sha256: rustSha },
      bun: { version: bunVersion, sha256: bunSha },
    }),
  )
  const metaPath = join(app, 'metadata.yml')
  writeFileSync(
    metaPath,
    replaceMetadata(readFileSync(metaPath, 'utf8'), {
      tag,
      rust: rustVersion,
      bun: bunVersion,
    }),
  )
  return { cargo: cargo.length, node: node.length, rustVersion, bunVersion }
}

function shaFrom(yml, urlNeedle) {
  const at = yml.indexOf(urlNeedle)
  if (at < 0) throw new Error(`manifest has no ${urlNeedle} source`)
  const match = yml.slice(at).match(/sha256: ([0-9a-f]{64})/)
  if (!match) throw new Error(`no sha256 after ${urlNeedle}`)
  return match[1]
}

function selfTest() {
  const hex = Buffer.from(
    '4xTZr1FUmSoQW4XIWmit3tzQrUTZM+N3P0XV8xROKYF50XfI7xeO90+1bZvNwxIufQ9hDQVRJH5YhgPVF8A/HQ==',
    'base64',
  ).toString('hex')
  if (
    hex !==
    'e314d9af5154992a105b85c85a68addedcd0ad44d933e3773f45d5f3144e298179d177c8ef178ef74fb56d9bcdc3122e7d0f610d0551247e588603d517c03f1d'
  ) {
    throw new Error(`sha512 hex mismatch: ${hex}`)
  }
  const here = dirname(fileURLToPath(import.meta.url))
  const root = join(here, '..')
  const cargo = cargoSources(readFileSync(join(root, 'desktop/src-tauri/Cargo.lock'), 'utf8'))
  const adler = cargo.find((s) => s.dest === 'cargo/vendor/adler2-2.0.1' && s.type === 'archive')
  if (adler?.sha256 !== '320119579fcad9c21884f5c4861d16174d0e06250625266f50fe6898340abefa') {
    throw new Error('cargo lock parse missed adler2')
  }
  const node = nodeSources(readFileSync(join(root, 'desktop/bun.lock'), 'utf8'))
  const esbuild = node.find((s) => s.dest === 'bun_cache/@esbuild/linux-x64@0.28.2@@@1')
  if (!esbuild || esbuild.sha512 !== hex) throw new Error('bun lock parse missed @esbuild/linux-x64')
  if (node.some((s) => s.dest.includes('darwin'))) throw new Error('darwin package leaked into linux sources')
  console.log(`self-test ok: ${cargo.length} cargo entries, ${node.length} bun packages`)
}

function arg(name) {
  const i = process.argv.indexOf(name)
  return i === -1 ? undefined : process.argv[i + 1]
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.includes('--self-test')) {
    selfTest()
  } else {
    const talaria = arg('--talaria')
    const omapak = arg('--omapak')
    const tag = arg('--tag')
    const commit = arg('--commit')
    if (!talaria || !omapak || !tag || !commit) {
      console.error('usage: omapak-desktop-submit.mjs --talaria DIR --omapak DIR --tag TAG --commit SHA [--fetch-shas]')
      process.exit(2)
    }
    const result = await submit({
      talaria,
      omapak,
      tag,
      commit,
      fetchShas: process.argv.includes('--fetch-shas'),
    })
    console.log(`updated ${APP} → ${tag} (${commit.slice(0, 12)}) ${JSON.stringify(result)}`)
  }
}
