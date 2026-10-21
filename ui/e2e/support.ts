// What every MCP e2e test leans on: where things are, an authenticated api
// client, the cleanup the suite owes the stack it ran on, and the locators
// for components that carry no test ids (the suite reads the page the way a
// person does: by the text on it).
import { readFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { expect, type APIRequestContext, type Locator, type Page } from '@playwright/test'

const here = import.meta.dirname

export const BASE_URL = process.env.E2E_BASE_URL || 'http://localhost:5303'
/** The stub process (scripts/e2e/mcp-stubs.mjs) — one origin for all of it. */
export const STUB_URL = (process.env.E2E_STUB_URL || 'https://127.0.0.1:5390').replace(/\/+$/, '')
export const STATE_FILE = join(process.env.E2E_STATE_DIR || join(here, '.state'), 'session.json')
/** The fleet env the api renders agent credentials into (fleet-layout:
 *  `<api cwd>/../fleet/.env`, or TALARIA_FLEET_DIR) — the same file an agent
 *  container reads its TALARIA_AGENT_KEY_<SLUG> from. */
export const FLEET_ENV = process.env.E2E_FLEET_ENV || resolve(here, '../../fleet/.env')

/** The stub's catalog, as the registry publishes it. Titles are what a
 *  person searches for; names are what the mirror keys on. */
export const STUB = {
  open: { name: 'e2e/open', title: 'E2E Open', url: `${STUB_URL}/open/mcp`, slug: 'e2e-open' },
  key: { name: 'e2e/key', title: 'E2E Key', url: `${STUB_URL}/key/mcp`, slug: 'e2e-key' },
  oauth: { name: 'e2e/oauth', title: 'E2E OAuth', url: `${STUB_URL}/oauth/mcp`, slug: 'e2e-oauth' },
  dead: { name: 'e2e/dead', title: 'E2E Dead' },
  pkg: { name: 'e2e/package', title: 'E2E Package' },
} as const
/** What /key/mcp accepts (scripts/e2e/mcp-stubs.mjs KEY). Never a substring
 *  of a server name — the credentials test asserts it never comes back. */
export const STUB_KEY = 'stub-secret-4f9c2a'
export const STUB_TOOLS = ['echo', 'shout', 'count']

/** Whether the api's catalog mirror is pointed at the stub registry
 *  (TALARIA_MCP_REGISTRY_URL). `E2E_MARKETPLACE=0` says it is not — an api
 *  built before that override existed, or one whose mirror must stay on the
 *  real registry: the catalog test then skips, and the install tests register
 *  the stub servers by URL with the exact body the marketplace would send,
 *  so everything after the install still runs. */
export const MARKETPLACE = process.env.E2E_MARKETPLACE !== '0'

/** A per-run suffix: names a rerun must not collide with. */
export const RUN = Date.now().toString(36)

// ── The api ─────────────────────────────────────────────────────────────────

export interface ServerRow {
  id: string
  name: string
  label: string
  url: string
  probe: { state: string; detail: string } | null
  tools: Array<{ name: string }>
  orgConnected: boolean | null
  oauthEnabled: boolean
  assignments: Array<{ agentModel: string; tools: string[] | null }>
}

/** JSON over the suite's session. Every failure names the call and the
 *  answer — a red test should read as the request that went wrong. */
export class Api {
  constructor(private readonly request: APIRequestContext) {}

  async call<T>(method: 'GET' | 'POST' | 'PUT' | 'DELETE', path: string, data?: unknown, headers?: Record<string, string>): Promise<T> {
    const res = await this.request.fetch(path, { method, data, headers, failOnStatusCode: false })
    const text = await res.text()
    if (!res.ok()) throw new Error(`${method} ${path} → ${res.status()}: ${text.slice(0, 400)}`)
    return (text ? JSON.parse(text) : null) as T
  }
  get<T>(path: string) {
    return this.call<T>('GET', path)
  }
  post<T>(path: string, data?: unknown) {
    return this.call<T>('POST', path, data ?? {})
  }
  put<T>(path: string, data: unknown) {
    return this.call<T>('PUT', path, data)
  }
  del<T>(path: string) {
    return this.call<T>('DELETE', path)
  }

  async servers(): Promise<ServerRow[]> {
    return (await this.get<{ servers: ServerRow[] }>('/api/mcp/servers')).servers
  }
  async server(id: string): Promise<ServerRow> {
    const s = (await this.servers()).find((x) => x.id === id)
    if (!s) throw new Error(`server ${id} is not in the registry`)
    return s
  }
  async serverNamed(name: string): Promise<ServerRow | undefined> {
    return (await this.servers()).find((x) => x.name === name)
  }

  /** Remove registry rows a previous (crashed) run left behind: by name, and
   *  any row pointing at the stub — the marketplace marks an entry installed
   *  when a row already has its URL, which would hide the install button. */
  async removeLeftovers(names: string[]) {
    for (const s of await this.servers()) {
      if (names.includes(s.name) || s.url.startsWith(STUB_URL)) await this.del(`/api/mcp/servers/${s.id}`)
    }
  }

  async removeServer(id: string | undefined) {
    if (!id) return
    try {
      await this.del(`/api/mcp/servers/${id}`)
    } catch (e) {
      // Already gone is the state cleanup wants.
      if (!String(e).includes('→ 404')) throw e
    }
  }
}

/** What the marketplace's install does (ui/src/routes/app/mcp.ts
 *  `installLibraryServer`), minus the catalog: register the entry under its
 *  slug, then run discovery unless the server turned out to speak OAuth.
 *  The fallback for a stack whose mirror is not on the stub registry. */
export async function installByUrl(api: Api, entry: { title: string; slug: string; url: string; description?: string }): Promise<string> {
  const { server } = await api.post<{ server: { id: string; oauthMeta: unknown } }>('/api/mcp/servers', {
    name: entry.slug,
    label: entry.title,
    url: entry.url,
    description: entry.description ?? null,
    requiredHeaders: [],
  })
  if (!server.oauthMeta) {
    // A refusal here is discovery's verdict (a 401 is "needs credentials"),
    // which the card then shows — not a failed install.
    await api.put(`/api/mcp/servers/${server.id}`, { refreshTools: true }).catch(() => {})
  }
  return server.id
}

// ── Agents ──────────────────────────────────────────────────────────────────

/** A hire needs a model to write into the agent's config (fleet-create
 *  refuses "no models configured"), and a fresh CI database has none. Adds a
 *  placeholder endpoint when no endpoint lists a model — nothing ever calls
 *  it, the agent is never started — and answers its id so the run removes it. */
export async function ensureLlmEndpoint(api: Api, name: string): Promise<string | null> {
  const { endpoints } = await api.get<{ endpoints: Array<{ models?: string[] }> }>('/api/fleet/endpoints')
  if (endpoints.some((e) => (e.models ?? []).length > 0)) return null
  const { id } = await api.post<{ id: string }>('/api/fleet/endpoints', {
    name,
    provider: 'openai',
    baseUrl: `${STUB_URL}/v1`,
    class: 'cloud',
    models: ['e2e-model'],
  })
  return id
}

export async function removeLlmEndpoint(api: Api, id: string | null) {
  if (!id) return
  try {
    await api.del(`/api/fleet/endpoints/${id}`)
  } catch (e) {
    if (!String(e).includes('→ 404')) throw e
  }
}

export interface AgentDefRow {
  id: string
  slug: string
  model: string
  displayName: string
  enabled: boolean
}

/** Hire an agent WITHOUT a container (start: false): the def, its first
 *  version and the fleet render are what the MCP surface needs; a booted
 *  Hermes is not. Answers once the def exists. */
export async function hireAgent(api: Api, slug: string, displayName: string): Promise<AgentDefRow> {
  await api.post('/api/fleet/create', { slug, department: 'e2e', displayName, role: 'End-to-end test agent', start: false })
  let def: AgentDefRow | undefined
  await expect
    .poll(
      async () => {
        const hires = (await api.get<{ hires: Array<{ slug: string; state: string; error: string | null }> }>('/api/fleet/hires')).hires
        const hire = hires.find((h) => h.slug === slug)
        if (hire?.state === 'error') throw new Error(`hiring ${slug} failed: ${hire.error}`)
        def = (await api.get<{ defs: AgentDefRow[] }>('/api/fleet/defs')).defs.find((d) => d.slug === slug)
        return !!def && (!hire || hire.state === 'done')
      },
      { message: `agent ${slug} never finished hiring`, timeout: 120_000, intervals: [1_000, 2_000] },
    )
    .toBe(true)
  return def!
}

/** Retire, then delete — the only order the control route allows. */
export async function removeAgent(api: Api, id: string | undefined) {
  if (!id) return
  for (const action of ['retire', 'delete']) {
    try {
      await api.post(`/api/fleet/agents/${id}/control`, { action })
    } catch (e) {
      if (String(e).includes('→ 404')) return
      throw e
    }
  }
}

/** The agent's own credential, the one its container presents to the
 *  gateway: rendered into the fleet env as TALARIA_AGENT_KEY_<SLUG>. Falls
 *  back to the org-wide key (E2E_AGENT_KEY + x-agent-name), which the
 *  gateway still accepts for an ordinary agent while the legacy window is
 *  open. Null when neither is available — the caller says so. */
export function agentCredential(def: AgentDefRow): { headers: Record<string, string>; how: string } | null {
  const varName = `TALARIA_AGENT_KEY_${def.slug.toUpperCase()}`
  try {
    const line = readFileSync(FLEET_ENV, 'utf8')
      .split('\n')
      .find((l) => l.startsWith(`${varName}=`))
    const key = line?.slice(varName.length + 1).trim()
    if (key) return { headers: { 'x-api-key': key }, how: `the agent's own key (${varName} in ${FLEET_ENV})` }
  } catch {
    // No fleet env readable from here — try the shared key.
  }
  const shared = process.env.E2E_AGENT_KEY
  if (shared) return { headers: { 'x-api-key': shared, 'x-agent-name': def.model }, how: 'the org-wide TALARIA_AGENT_KEY (legacy window)' }
  return null
}

// ── Locators ────────────────────────────────────────────────────────────────

const xq = (s: string) => (s.includes('"') ? `concat("${s.split('"').join('", \'"\', "')}")` : `"${s}"`)

/** A server card on Manage → MCP: the nearest box around the label that
 *  also holds the card's "Manage access" door. */
export function registryCard(page: Page, label: string): Locator {
  return page.locator(
    `xpath=//*[normalize-space(text())=${xq(label)}]/ancestor::*[.//button[normalize-space()="Manage access"]][1]`,
  )
}

/** A marketplace entry: the nearest box around the title that holds its
 *  install control (a button before install, the check mark after). */
export function marketCard(scope: Locator, title: string): Locator {
  return scope.locator(
    `xpath=.//*[normalize-space(text())=${xq(title)}]/ancestor::*[.//button or .//*[@title="In your org registry"]][1]`,
  )
}

/** One server row in an agent's MCP tab: the nearest box around the label
 *  that holds its toggle. */
export function agentServerRow(scope: Locator, label: string): Locator {
  return scope.locator(`xpath=.//*[normalize-space(text())=${xq(label)}]/ancestor::*[.//*[@role="switch"]][1]`)
}

/** The marketplace dialog, opened from Manage → MCP → External. */
export async function openMarketplace(page: Page): Promise<Locator> {
  await page.goto('/mcp/external')
  await page.getByRole('button', { name: /Browse marketplace/ }).first().click()
  const dialog = page.getByRole('dialog').filter({ hasText: 'Custom / self-hosted server' })
  await expect(dialog).toBeVisible()
  return dialog
}

/** Type into the marketplace search (the dialog's first text field). */
export async function searchMarketplace(dialog: Locator, q: string) {
  const box = dialog.getByRole('textbox').first()
  await box.fill(q)
}

/** Install one entry from an open marketplace, one click (entries the stub
 *  publishes declare no credentials, so none opens the install dialog). */
export async function installFromMarketplace(dialog: Locator, title: string): Promise<Locator> {
  await searchMarketplace(dialog, title)
  const card = marketCard(dialog, title)
  await expect(card).toBeVisible()
  await card.hover()
  await card.getByRole('button', { name: /^Add to your org/ }).click()
  return card
}
