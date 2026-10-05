// Exercises EVERY provider on the bridge's OAuth roster, one login start each,
// and reports what shape came back.
//
// Not a unit test and deliberately not in CI: every flow here talks to a real
// provider (an authorize URL is built locally, but a device-code flow
// registers with the provider to get a user code, and a custom flow may POST
// before it can tell you anything). It is the manual gate for "the bridge
// still drives all of them" — run it after bumping @oh-my-pi/pi-ai, and read
// the table.
//
// It never completes a login: each session is started, observed until it is
// waiting on a person, and then cancelled. Nothing is stored.
//
//   bun scripts/probe-flows.ts            # the whole roster
//   bun scripts/probe-flows.ts anthropic  # one or more provider ids

const PORT = Number.parseInt(process.env.PROBE_PORT ?? '5298', 10)
const TOKEN = 'probe-' + crypto.randomUUID()
const BASE = `http://127.0.0.1:${PORT}`
const AUTH = { Authorization: `Bearer ${TOKEN}` }
/** Long enough for a device-code registration round trip on a slow network. */
const PER_PROVIDER_MS = 20_000

const entry = new URL('../dist/server.js', import.meta.url).pathname

const child = Bun.spawn(['bun', entry], {
  env: { ...process.env, OMP_AUTH_BRIDGE_PORT: String(PORT), OMP_AUTH_BRIDGE_TOKEN: TOKEN },
  stdout: 'pipe',
  stderr: 'pipe',
})

async function waitForBridge(): Promise<void> {
  for (let i = 0; i < 100; i++) {
    try {
      const r = await fetch(`${BASE}/healthz`)
      if (r.ok) return
    } catch {}
    await Bun.sleep(100)
  }
  throw new Error('bridge did not come up')
}

type Wire = {
  id: string
  phase: string
  url: string | null
  instructions: string | null
  progress: string | null
  prompt: { kind: string; message: string } | null
  error: string | null
}

/** What a person would be looking at. "ready" means the flow can be finished. */
function verdict(w: Wire): { ok: boolean; shape: string; detail: string } {
  if (w.error) return { ok: false, shape: 'error', detail: w.error }
  const bits: string[] = []
  if (w.url) bits.push('url')
  if (w.prompt) bits.push(`prompt:${w.prompt.kind}`)
  if (w.instructions?.match(/code:?\s*[A-Z0-9-]{4,}/i)) bits.push('user-code')
  if (bits.length === 0) return { ok: false, shape: 'nothing', detail: w.progress ?? w.phase }
  return {
    ok: true,
    shape: bits.join('+'),
    detail: w.prompt?.message ?? w.instructions ?? w.progress ?? '',
  }
}

await waitForBridge()

const rosterRes = await fetch(`${BASE}/v1/providers`, { headers: AUTH })
const { providers } = (await rosterRes.json()) as {
  providers: { id: string; name: string; flow: string; pasteCode: boolean; storeAs: string }[]
}
const only = process.argv.slice(2)
const targets = only.length > 0 ? providers.filter(p => only.includes(p.id)) : providers

console.log(`roster: ${providers.length} oauth providers; probing ${targets.length}\n`)

const rows: { id: string; flow: string; ok: boolean; shape: string; detail: string }[] = []

for (const p of targets) {
  const started = await fetch(`${BASE}/v1/login`, {
    method: 'POST',
    headers: { ...AUTH, 'content-type': 'application/json' },
    body: JSON.stringify({ provider: p.id }),
  })
  if (!started.ok) {
    rows.push({
      id: p.id,
      flow: p.flow,
      ok: false,
      shape: 'start-refused',
      detail: ((await started.json()) as { error?: string }).error ?? String(started.status),
    })
    continue
  }
  let w = (await started.json()) as Wire
  const deadline = Date.now() + PER_PROVIDER_MS
  // Poll until the flow is waiting on a person (a URL, a code, or a prompt)
  // or it fails. A GET consumes a finished session, which cannot happen here:
  // no login completes without the person.
  while (Date.now() < deadline) {
    const r = await fetch(`${BASE}/v1/login/${w.id}`, { headers: AUTH })
    if (!r.ok) break
    w = (await r.json()) as Wire
    if (w.error || w.url || w.prompt) break
    await Bun.sleep(250)
  }
  const v = verdict(w)
  rows.push({ id: p.id, flow: p.flow, ...v })
  await fetch(`${BASE}/v1/login/${w.id}`, { method: 'DELETE', headers: AUTH }).catch(() => {})
  console.log(
    `${v.ok ? '✓' : '✗'} ${p.id.padEnd(22)} ${p.flow.padEnd(12)} ${v.shape.padEnd(20)} ${v.detail.slice(0, 70)}`,
  )
}

child.kill()

const bad = rows.filter(r => !r.ok)
console.log(`\n${rows.length - bad.length}/${rows.length} reached a state a person can act on`)
if (bad.length > 0) {
  console.log('not reached:')
  for (const r of bad) console.log(`  ${r.id} (${r.flow}): ${r.shape} — ${r.detail.slice(0, 160)}`)
}
process.exit(bad.length > 0 ? 1 : 0)
