// The MCP surface end to end — catalog, install, credentials, OAuth, an
// agent's MCP tab, the gateway, and the custom form — against a real stack
// and the stub world in scripts/e2e/mcp-stubs.mjs. docs/E2E.md says how to
// run it; docs/MCP.md is the behaviour it holds the product to.
//
// Seven chapters, run in order by one worker — but NOT serial: a red chapter
// must not hide the six after it. Playwright answers a failure with a fresh
// worker (afterAll cleans up, beforeAll runs again, module state is new), so
// a chapter that builds on another (5 assigns the open server, 6 calls it as
// the agent) asks for what it needs through `ensureOpenServer` /
// `ensureAgent`: reused when an earlier chapter made it in this worker,
// made by the api otherwise. Each chapter names its own servers, and afterAll
// removes everything the run created, so a rerun starts from the same
// registry.
import { expect, test, type Page } from '@playwright/test'
import {
  Api,
  BASE_URL,
  MARKETPLACE,
  RUN,
  STATE_FILE,
  STUB,
  STUB_KEY,
  STUB_TOOLS,
  STUB_URL,
  agentCredential,
  agentServerRow,
  ensureLlmEndpoint,
  hireAgent,
  installByUrl,
  installFromMarketplace,
  marketCard,
  openMarketplace,
  registryCard,
  removeAgent,
  removeLlmEndpoint,
  searchMarketplace,
  type AgentDefRow,
} from './support'

const created = {
  servers: new Map<string, string>(), // name → id
  agent: undefined as AgentDefRow | undefined,
  endpoint: null as string | null,
}
const CUSTOM = { label: `E2E Custom ${RUN}`, slug: `e2e-custom-${RUN}` }
// An agent slug is `^[a-z][a-z0-9]{1,30}$` — no dash (talaria-agent-defs).
const AGENT = { slug: `e2e${RUN}`, displayName: `E2E Agent ${RUN}` }

let api: Api

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE_URL, storageState: STATE_FILE, ignoreHTTPSErrors: true })
  api = new Api(ctx)
  await api.removeLeftovers([STUB.open.slug, STUB.key.slug, STUB.oauth.slug])
})

test.afterAll(async () => {
  for (const id of created.servers.values()) await api.removeServer(id)
  // By slug too: a hire that failed half-way leaves a def no chapter holds.
  const agentId =
    created.agent?.id ?? (await api.get<{ defs: AgentDefRow[] }>('/api/fleet/defs').catch(() => ({ defs: [] }))).defs.find((d) => d.slug === AGENT.slug)?.id
  await removeAgent(api, agentId)
  await removeLlmEndpoint(api, created.endpoint)
})

/** Install one stub server the way this run can: from the marketplace when
 *  the mirror carries the stub registry, else by URL with the marketplace's
 *  own request body (support.ts `installByUrl`). Answers the marketplace card
 *  (null on the URL path) and leaves the page on Manage → MCP. */
async function install(page: Page, entry: { title: string; slug: string; url: string }) {
  if (MARKETPLACE) {
    const dialog = await openMarketplace(page)
    return { dialog, card: await installFromMarketplace(dialog, entry.title) }
  }
  test.info().annotations.push({ type: 'install', description: 'by URL (E2E_MARKETPLACE=0): the mirror is not on the stub registry' })
  await installByUrl(api, { ...entry, description: `Talaria e2e stub (${entry.slug}).` })
  await page.goto('/mcp/external')
  return { dialog: null, card: null }
}

/** The registry id of a server the run installed — and remember it for the
 *  cleanup, whichever chapter created it. */
async function track(name: string): Promise<string> {
  let id: string | undefined
  await expect.poll(async () => (id = (await api.serverNamed(name))?.id), { message: `${name} never reached the registry` }).toBeTruthy()
  created.servers.set(name, id!)
  return id!
}

/** The open stub server, connected: chapter 2's when it ran in this worker,
 *  else registered by URL the way the marketplace would. */
async function ensureOpenServer(): Promise<string> {
  const have = created.servers.get(STUB.open.slug)
  if (have && (await api.serverNamed(STUB.open.slug))?.id === have) return have
  test.info().annotations.push({ type: 'setup', description: 'E2E Open registered by URL (chapter 2 did not leave it in this worker)' })
  await installByUrl(api, { ...STUB.open, description: 'Talaria e2e stub (e2e-open).' })
  const id = await track(STUB.open.slug)
  await expect.poll(async () => (await api.server(id)).probe?.state, { message: 'discovery never ran on E2E Open' }).toBe('ok')
  return id
}

/** The run's agent: chapter 5's when it ran in this worker, else hired now. */
async function ensureAgent(): Promise<AgentDefRow> {
  if (created.agent) return created.agent
  created.endpoint = await ensureLlmEndpoint(api, `e2e-llm-${RUN}`)
  created.agent = await hireAgent(api, AGENT.slug, AGENT.displayName)
  return created.agent
}

async function closeDialogs(page: Page) {
  // Escape closes the top-most dialog; the marketplace may hold one more.
  for (let i = 0; i < 3 && (await page.getByRole('dialog').count()) > 0; i++) await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toHaveCount(0)
}

/** The tool strip and status chip of a registry card, after a discovery. */
async function expectReadyCard(page: Page, label: string, status: string) {
  const card = registryCard(page, label)
  await expect(card).toBeVisible()
  await expect(card.getByRole('button', { name: new RegExp(`^${STUB_TOOLS.length} tools`) })).toBeVisible()
  for (const t of STUB_TOOLS) await expect(card.getByText(t, { exact: true })).toBeVisible()
  await expect(card.getByText(status, { exact: true })).toBeVisible()
}

test('1 · the catalog syncs from the stub registry and the marketplace finds it', async ({ page }) => {
  test.skip(!MARKETPLACE, 'E2E_MARKETPLACE=0: the api mirror is not pointed at the stub registry (TALARIA_MCP_REGISTRY_URL)')
  await api.post('/api/mcp/library/sync')
  // Ready AND carrying the stub: a stack whose mirror was already ready
  // answers "ready" before the pass that brings the stub entries lands.
  type Hit = { registryName: string; title: string; package: unknown; live?: { state: string } | null }
  type Library = { servers: Hit[]; catalog: { phase: string; error: string | null } }
  // includeDead: the liveness job (first pass minutes after boot, then every
  // five) marks e2e/dead dead, and the default search then hides it. With
  // includeDead the mirror answers for every entry whenever that pass lands.
  let lib: Library | undefined
  const deadline = Date.now() + 150_000
  for (;;) {
    lib = await api.get<Library>('/api/mcp/library?q=E2E&limit=50&includeDead=1')
    const names = lib.servers.map((s) => s.registryName)
    if (lib.catalog.phase === 'ready' && [STUB.open, STUB.key, STUB.oauth, STUB.dead, STUB.pkg].every((s) => names.includes(s.name))) break
    if (Date.now() > deadline) {
      throw new Error(
        `the mirror never carried the stub entries (catalog: ${JSON.stringify(lib.catalog)}; e2e names found: ${JSON.stringify(names.filter((n) => n.startsWith('e2e/')))}). ` +
          `Is the api started with TALARIA_MCP_REGISTRY_URL=${STUB_URL}/v0.1/servers (and the stub's SSL_CERT_FILE / TALARIA_FETCH_ALLOW_HOSTS)?`,
      )
    }
    await new Promise((r) => setTimeout(r, 2_500))
  }
  // `version=latest`: the superseded version of e2e/open is not in the mirror.
  expect(lib!.servers.filter((s) => s.registryName === STUB.open.name).map((s) => s.title)).toEqual([STUB.open.title])
  expect(lib!.servers.find((s) => s.registryName === STUB.pkg.name)?.package).toBeTruthy()
  // The three live servers are never judged dead (a 401 is alive: auth).
  for (const s of [STUB.open, STUB.key, STUB.oauth]) {
    expect(lib!.servers.find((h) => h.registryName === s.name)?.live?.state ?? 'unchecked', `${s.name} liveness`).not.toBe('dead')
  }
  // e2e/dead is either not checked yet (listed) or dead — and then hidden
  // from the default search. Read in that order: a row only ever turns dead,
  // so a dead verdict seen first must hold for the plain search after it.
  if (lib!.servers.find((h) => h.registryName === STUB.dead.name)?.live?.state === 'dead') {
    const plain = await api.get<Library>('/api/mcp/library?q=E2E&limit=50')
    expect(plain.servers.map((h) => h.registryName)).not.toContain(STUB.dead.name)
  }

  const dialog = await openMarketplace(page)
  await searchMarketplace(dialog, 'E2E')
  // Not e2e/dead: whether it shows depends on when the liveness pass ran.
  for (const s of [STUB.open, STUB.key, STUB.oauth, STUB.pkg]) {
    await expect(marketCard(dialog, s.title)).toBeVisible()
  }
  await expect(dialog.getByText('E2E Open (old)', { exact: true })).toHaveCount(0)
  // The package entry says what it is before anyone installs it.
  await expect(marketCard(dialog, STUB.pkg.title).getByText('pkg', { exact: true })).toBeVisible()
})

test('2 · installing an open server from the marketplace lands it ready, with its tools', async ({ page }) => {
  const { card } = await install(page, STUB.open)
  if (card) await expect(card.getByTitle('In your org registry')).toBeVisible()
  const id = await track(STUB.open.slug)
  await expect.poll(async () => (await api.server(id)).probe?.state, { message: 'discovery never ran on install' }).toBe('ok')
  const row = await api.server(id)
  expect(row.tools.map((t) => t.name).sort()).toEqual([...STUB_TOOLS].sort())

  await closeDialogs(page)
  await expectReadyCard(page, STUB.open.title, 'ready')
})

test('3 · a server that answers 401 asks for its key, and connects once it has it', async ({ page }) => {
  await install(page, STUB.key)
  const id = await track(STUB.key.slug)
  await expect.poll(async () => (await api.server(id)).probe?.state, { message: 'discovery never recorded the 401' }).toBe('auth-required')
  const creds = page.getByRole('dialog').filter({ hasText: `Credentials for ${STUB.key.title}` })
  if (!MARKETPLACE) {
    // No marketplace to open the editor on its own: the card says what the
    // server needs, and its one button is the way in.
    const card = registryCard(page, STUB.key.title)
    await expect(card.getByText('needs credentials', { exact: true })).toBeVisible()
    await card.getByRole('button', { name: 'Set credentials' }).click()
  }
  // From the marketplace this is the drop-in moment: the editor opens itself.
  await expect(creds).toBeVisible()
  await expect(creds.getByText(/requires authentication/)).toBeVisible()

  await creds.getByLabel('Header name').first().fill('X-Api-Key')
  await creds.getByLabel('Header value').first().fill(STUB_KEY)
  await creds.getByRole('button', { name: 'Save and test' }).click()
  await expect(creds).toHaveCount(0)

  const row = await api.server(id)
  expect(row.probe?.state).toBe('ok')
  expect(row.tools).toHaveLength(STUB_TOOLS.length)
  await closeDialogs(page)
  await expectReadyCard(page, STUB.key.title, 'ready')
  // Header values never come back — a key name, a mask.
  expect(JSON.stringify(await api.servers())).not.toContain(STUB_KEY)
})

test('4 · an OAuth server is detected, connects through the popup, and lists its tools', async ({ page }) => {
  const { card } = await install(page, STUB.oauth)
  const id = await track(STUB.oauth.slug)
  const before = await api.server(id)
  expect(before.oauthEnabled).toBe(true)
  expect(before.orgConnected).toBeFalsy()

  // Connect → /api/mcp/oauth/start → the stub's /authorize (302, no login
  // page) → Talaria's callback, which exchanges the code with PKCE and
  // closes the popup.
  const popupOpened = page.waitForEvent('popup')
  if (card) await card.getByRole('button', { name: 'Connect org account' }).click()
  else await registryCard(page, STUB.oauth.title).getByRole('button', { name: 'Connect', exact: true }).click()
  const popup = await popupOpened
  await popup.waitForURL(/\/api\/mcp\/oauth\/callback/, { waitUntil: 'domcontentloaded' })
  // The page closes itself 1.2s after it renders; read it while it is there.
  const said = await popup
    .locator('body')
    .innerText()
    .catch(() => '(the popup closed before it could be read)')
  expect(said, 'the OAuth callback page').not.toContain('Connection failed')
  await expect.poll(async () => (await api.server(id)).orgConnected, { message: 'the callback never stored the org token' }).toBe(true)

  await closeDialogs(page)
  const reg = registryCard(page, STUB.oauth.title)
  await expect(reg.getByText('connected', { exact: true })).toBeVisible()
  // Tools come from the server once the bearer exists — ask for them.
  await reg.getByRole('button', { name: /Discover tools|tools/ }).first().click()
  await expectReadyCard(page, STUB.oauth.title, 'connected')
  expect((await api.server(id)).tools).toHaveLength(STUB_TOOLS.length)
})

test("5 · an agent's MCP tab assigns a server with a tool subset, and takes it away again", async ({ page }) => {
  const openId = await ensureOpenServer()
  const agent = await ensureAgent()
  const agentId = agent.id
  // Every edit the tab makes would also ROLL the agent (`apply` defaults to
  // true): a blue-green container swap that pulls the Hermes image and joins
  // the fleet network — the agent was hired without a container on purpose,
  // and a roll is not what this chapter is about. The tab's requests go out
  // exactly as the UI built them, plus `apply: false`; the assignment, the
  // tool subset and the fleet render still happen, so what the gateway
  // enforces is the real thing.
  const sent: unknown[] = []
  await page.route(`**/api/fleet/defs/${agentId}/mcp`, async (route) => {
    const req = route.request()
    if (req.method() !== 'POST') return route.continue()
    const body = (req.postDataJSON() ?? {}) as Record<string, unknown>
    sent.push(body)
    await route.continue({ postData: JSON.stringify({ ...body, apply: false }) })
  })
  type AgentMcp = { servers: Array<{ id: string; carried: boolean; assigned: boolean; assignedTools: string[] | null }> }
  const onAgent = async () => (await api.get<AgentMcp>(`/api/fleet/defs/${agentId}/mcp`)).servers.find((s) => s.id === openId)

  // One visit to the roster, one modal, for the whole chapter: the registry
  // card is read in a second tab beside it. (Loading /agents again after an
  // agent modal was open left the roster on its skeletons on the dev stack
  // this suite was written against — a roster bug, not the MCP surface.)
  await page.goto('/agents')
  await page.getByRole('button', { name: new RegExp(AGENT.displayName) }).first().click()
  const modal = page.getByRole('dialog').filter({ hasText: AGENT.displayName })
  await modal.getByRole('button', { name: 'MCP', exact: true }).click()
  await expect(modal.getByText('On this agent')).toBeVisible()
  const registryCount = async (text: RegExp) => {
    const tab = await page.context().newPage()
    try {
      await tab.goto('/mcp/external')
      await expect(registryCard(tab, STUB.open.title).getByText(text)).toBeVisible()
    } finally {
      await tab.close()
    }
  }

  let row = agentServerRow(modal, STUB.open.title)
  await expect(row.getByRole('switch')).toHaveAttribute('aria-checked', 'false')
  await row.getByRole('switch').click()
  await expect.poll(async () => (await onAgent())?.assigned, { message: 'the toggle never assigned the server' }).toBe(true)
  // The row moves up to "On this agent", switched on.
  row = agentServerRow(modal, STUB.open.title)
  await expect(row.getByRole('switch')).toHaveAttribute('aria-checked', 'true')

  // Narrow to one tool: the picker starts at "All tools".
  await row.getByRole('button', { name: /All tools/ }).click()
  await page.getByRole('option', { name: 'echo' }).click()
  // Close the picker with a click outside it, inside the modal: Escape is
  // the modal's own close once the picker has let go of focus.
  await modal.getByText('On this agent').click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await expect.poll(async () => (await onAgent())?.assignedTools, { message: 'the subset never saved' }).toEqual(['echo'])
  // What the tab asked for, in order: the server with every tool, then echo.
  expect(sent.slice(0, 2)).toEqual([{ assign: [{ serverId: openId, tools: null }] }, { assign: [{ serverId: openId, tools: ['echo'] }] }])

  // The registry card counts the assignment.
  await registryCount(/\b1 agent\b/)
  expect((await api.server(openId)).assignments).toEqual([{ agentModel: agent.model, tools: ['echo'] }])

  // And off again, from the same modal.
  await expect(modal).toBeVisible()
  row = agentServerRow(modal, STUB.open.title)
  await row.getByRole('switch').click()
  await expect.poll(async () => (await onAgent())?.carried, { message: 'the toggle never removed the server' }).toBe(false)
  await expect(agentServerRow(modal, STUB.open.title).getByRole('switch')).toHaveAttribute('aria-checked', 'false')
  await registryCount(/No agents yet/)
  await closeDialogs(page)
})

test('6 · the gateway lets the agent call an allowed tool and refuses the rest', async ({ playwright }) => {
  const openId = await ensureOpenServer()
  const def = await ensureAgent()
  // apply: false — no container roll (chapter 5 says why); the render that
  // writes the agent's key and its allowance still runs.
  await api.post(`/api/fleet/defs/${def.id}/mcp`, { assign: [{ serverId: openId, tools: ['echo'] }], apply: false })

  const cred = agentCredential(def)
  if (!cred) {
    // No credential reachable from this machine: hold the allowance the
    // gateway enforces instead, and say the call itself went unproven.
    test.info().annotations.push({ type: 'limitation', description: 'no agent credential readable (E2E_FLEET_ENV / E2E_AGENT_KEY): the gateway call was not made; the allowance was checked through the api' })
    const row = (await api.get<{ servers: Array<{ id: string; assignedTools: string[] | null }> }>(`/api/fleet/defs/${def.id}/mcp`)).servers.find((s) => s.id === openId)
    expect(row?.assignedTools).toEqual(['echo'])
    return
  }
  test.info().annotations.push({ type: 'credential', description: cred.how })

  // An agent presents its credential, never a session: a fresh context
  // with no cookies, so the gateway can only know the caller by its key.
  const agent = await playwright.request.newContext({ baseURL: BASE_URL, ignoreHTTPSErrors: true, storageState: { cookies: [], origins: [] } })
  const stub = await playwright.request.newContext({ ignoreHTTPSErrors: true })
  try {
    let id = 0
    const rpc = async (method: string, params?: unknown) => {
      const res = await agent.post(`/api/mcp/gw/${STUB.open.slug}`, {
        headers: { ...cred.headers, accept: 'application/json, text/event-stream' },
        data: { jsonrpc: '2.0', id: ++id, method, params },
      })
      expect(res.status(), `${method} through the gateway: ${await res.text()}`).toBe(200)
      const text = await res.text()
      // JSON, or the last data frame of an SSE answer.
      const frame = text.trim().startsWith('{') ? text : (text.split('\n').filter((l) => l.startsWith('data:')).pop() ?? '').slice(5)
      return JSON.parse(frame) as { result?: { tools?: Array<{ name: string }>; content?: Array<{ text: string }> }; error?: { code: number; message: string } }
    }

    const init = await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'e2e', version: '1' } })
    expect(init.error).toBeUndefined()
    const list = await rpc('tools/list', {})
    expect(list.result?.tools?.map((t) => t.name)).toEqual(['echo'])

    const said = `hello from ${RUN}`
    const echo = await rpc('tools/call', { name: 'echo', arguments: { text: said } })
    expect(echo.error).toBeUndefined()
    expect(echo.result?.content?.[0]?.text).toBe(said)

    const shout = await rpc('tools/call', { name: 'shout', arguments: { text: said } })
    expect(shout.error?.code).toBe(-32602)
    expect(shout.error?.message).toContain('not available here')

    // The refusal happened at the gateway: the upstream saw the echo, never the shout.
    const calls = (await (await stub.get(`${STUB_URL}/_e2e/calls`)).json()) as { calls: Array<{ server: string; tool: string; text: string }> }
    const ours = calls.calls.filter((c) => c.text === said)
    expect(ours.map((c) => `${c.server}.${c.tool}`)).toEqual(['open.echo'])

    // A caller with no credential gets nothing at all.
    const anon = await agent.post(`/api/mcp/gw/${STUB.open.slug}`, { data: { jsonrpc: '2.0', id: 99, method: 'tools/list' } })
    expect(anon.status()).toBe(401)
  } finally {
    await agent.dispose()
    await stub.dispose()
  }
})

test('7 · the custom form registers a raw URL and discovery proves it', async ({ page }) => {
  await page.goto('/mcp/external')
  await page.getByRole('button', { name: 'Custom server', exact: true }).click()
  const form = page.getByRole('dialog').filter({ hasText: 'Custom MCP server' })
  await form.getByLabel('Name', { exact: true }).fill(CUSTOM.label)
  await form.getByLabel(/^Server URL/).fill(STUB.open.url)
  await form.getByLabel('Description', { exact: true }).fill('Registered by the e2e suite through the custom form.')
  await form.getByRole('button', { name: 'Register server' }).click()
  await expect(form).toHaveCount(0)
  const id = await track(CUSTOM.slug)
  await expect.poll(async () => (await api.server(id)).probe?.state, { message: 'discovery never ran on the custom server' }).toBe('ok')
  await expectReadyCard(page, CUSTOM.label, 'ready')
})
