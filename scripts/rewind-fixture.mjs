#!/usr/bin/env node
// A real work session on a worktree stack, end to end: a Developer Agent on a
// real model through the Talaria gateway, doing a repo job on a throwaway
// Forgejo. It is the fixture the checkpoint-rewind lanes run, so what a session
// needs to be healthy is written down here and nowhere else.
//
//   node scripts/rewind-fixture.mjs setup    --stack <checkout> [--agent atlas] [--model <id>]
//   node scripts/rewind-fixture.mjs ticket   --stack <checkout> [--agent atlas] [--no-wait]
//   node scripts/rewind-fixture.mjs teardown --stack <checkout>
//
// `<checkout>` is a checkout made by `bun talaria worktree`, with `talaria dev`
// running in it. Setup is idempotent. `ticket` prints one JSON line and exits 0
// only when the session reached review with a pushed branch.
//
// WHY THE FORGEJO SITS BEHIND TLS ON :443. Talaria hands a git credential only
// over https, and the rendered pre-push hook breaks on a remote URL that carries
// a port. A real host has neither problem, so the fixture looks like one. The
// api reaches Forgejo's REST side on loopback, which `api_origin` allows.
//
// WHY A FREE MODEL BY DEFAULT. OpenRouter answers 402 when the account's credit
// cannot cover a request's worst-case output, and Hermes sends no `max_tokens`.
// A nearly empty account therefore fails every agent turn while a small curl
// succeeds. `preflight` sends the request shape an agent sends.

import { execFileSync } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { parseArgs } from 'node:util'

const DEFAULT_MODEL = 'stealth/crouton-alpha'
const ENDPOINT = 'OpenRouter'
const FLEET_NETWORK = 'talaria'
const FORGEJO_IMAGE = 'codeberg.org/forgejo/forgejo:11'
const TLS_IMAGE = 'nginx:1.27-alpine'
const HOST_ID = 'forgejo'
const REPO_OWNER = 'fixture'
const REPO_NAME = 'rewind-fixture'
const REPO = `${REPO_OWNER}/${REPO_NAME}`
const SECRET_NAME = 'forgejo-fixture'
const BOARD_NAME = 'Rewind fixture'
const CODE_ROLES = ['utility', 'code-light', 'code-standard', 'code-heavy']
const SESSION_DEADLINE_MS = 20 * 60_000

const TICKET = {
  title: 'Add subtract and multiply to the fixture calculator',
  priority: 'medium',
  description: `Repository: ${REPO} on the ${HOST_ID} host. Use a light-effort workbench job.

Do these in order, one at a time:
1. Add a \`subtract(a, b)\` function to \`calc.py\`.
2. Add a \`multiply(a, b)\` function to \`calc.py\`.
3. Create \`test_calc.py\` with one plain \`assert\` test per function, runnable with \`python3 test_calc.py\`.
4. Add a \`Usage\` section to \`README.md\` that lists the three functions.
5. Run \`python3 test_calc.py\` and confirm it passes.

Commit on the job's branch, finish the job, then move this ticket to review.`,
}

const sh = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'], ...opts }).trim()
const shOk = (cmd, args) => {
  try {
    return sh(cmd, args)
  } catch {
    return null
  }
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const say = (line) => process.stderr.write(`[fixture] ${line}\n`)
const die = (line) => {
  process.stderr.write(`[fixture] ${line}\n`)
  process.exit(1)
}

function readStack(checkout) {
  const envPath = join(checkout, 'ui/.env')
  if (!existsSync(envPath)) die(`${checkout} has no ui/.env: cut it with \`bun talaria worktree <name>\``)
  const env = {}
  for (const line of readFileSync(envPath, 'utf8').split('\n')) {
    const m = line.match(/^([A-Z0-9_]+)=(.*)$/)
    if (m && !(m[1] in env)) env[m[1]] = m[2].replace(/^["']|["']$/g, '')
  }
  const name = env.TALARIA_WORKTREE
  if (!name) die(`${checkout} is not a worktree stack (no TALARIA_WORKTREE in ui/.env)`)
  const slot = Number(env.TALARIA_API_PORT) - 5400
  return {
    checkout,
    name,
    app: `http://localhost:${env.PORT}`,
    pg: `talaria-pg-${name}`,
    redis: `talaria-redis-${name}`,
    fleetProject: `talaria-wt-${name}-fleet`,
    forgejoApp: `talaria-forgejo-${name}`,
    forgejoTls: `talaria-forgejo-tls-${name}`,
    forgejoHost: `forgejo-${name}`,
    forgejoAppHost: `forgejo-${name}-app`,
    forgejoRest: `http://127.0.0.1:${5900 + slot}`,
    forgejoPort: 5900 + slot,
    state: join(checkout, '.rewind-fixture'),
  }
}

const psql = (stack, sql) =>
  sh('docker', ['exec', stack.pg, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', sql])

/** The cookie carries an opaque id, so a session is one Redis key (repo-traps 5). */
function mintSession(stack) {
  const row = psql(
    stack,
    `select row_to_json(u) from (select id, email, name from users where role = 'admin' order by created_at limit 1) u`,
  )
  if (!row) die('the stack has no admin user to act as')
  const user = JSON.parse(row)
  const sid = `fixture-${randomBytes(12).toString('hex')}`
  const record = {
    id: user.id,
    sub: user.id,
    email: user.email,
    name: user.name ?? 'Rewind fixture',
    picture: null,
    role: 'admin',
    provider: 'password',
  }
  sh('docker', ['exec', stack.redis, 'redis-cli', 'SET', `sess:${sid}`, JSON.stringify(record), 'EX', '86400'])
  return sid
}

function apiClient(stack, sid) {
  return async (method, path, body) => {
    const res = await fetch(`${stack.app}${path}`, {
      method,
      headers: { cookie: `talaria_session=${sid}`, 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const text = await res.text()
    if (!res.ok) throw new Error(`${method} ${path} → ${res.status}: ${text.slice(0, 300)}`)
    return text ? JSON.parse(text) : {}
  }
}

function agentRow(stack, slug) {
  const row = psql(
    stack,
    `select row_to_json(a) from (select id, model, department from agent_defs where slug = '${slug}') a`,
  )
  if (!row) die(`the stack has no agent "${slug}"`)
  return JSON.parse(row)
}

const agentContainer = (stack) =>
  shOk('docker', [
    'ps',
    '--filter',
    `label=com.docker.compose.project=${stack.fleetProject}`,
    '--format',
    '{{.Names}}\t{{.Status}}',
  ])
    ?.split('\n')
    .filter(Boolean)
    .map((l) => l.split('\t'))
    .find(([, status]) => status.includes('healthy') && !status.includes('unhealthy'))?.[0] ?? null

async function forgejoFetch(stack, token, method, path, body) {
  const res = await fetch(`${stack.forgejoRest}/api/v1${path}`, {
    method,
    headers: { authorization: `token ${token}`, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  return { status: res.status, json: await res.json().catch(() => null) }
}

async function ensureForgejo(stack) {
  mkdirSync(stack.state, { recursive: true })
  const cert = join(stack.state, 'tls.crt')
  const key = join(stack.state, 'tls.key')
  if (!existsSync(cert)) {
    sh('openssl', [
      'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30',
      '-keyout', key, '-out', cert,
      '-subj', `/CN=${stack.forgejoHost}`,
      '-addext', `subjectAltName=DNS:${stack.forgejoHost}`,
    ])
  }
  writeFileSync(
    join(stack.state, 'nginx.conf'),
    `events {}
http {
  client_max_body_size 256m;
  server {
    listen 443 ssl;
    ssl_certificate /tls/tls.crt;
    ssl_certificate_key /tls/tls.key;
    location / {
      proxy_pass http://${stack.forgejoAppHost}:3000;
      proxy_set_header Host $host;
      proxy_set_header X-Forwarded-Proto https;
    }
  }
}
`,
  )
  const running = (name) => shOk('docker', ['inspect', '-f', '{{.State.Running}}', name]) === 'true'
  if (!running(stack.forgejoApp)) {
    shOk('docker', ['rm', '-f', stack.forgejoApp])
    sh('docker', [
      'run', '-d', '--name', stack.forgejoApp,
      '--network', FLEET_NETWORK, '--network-alias', stack.forgejoAppHost,
      '-p', `127.0.0.1:${stack.forgejoPort}:3000`,
      '-e', 'USER_UID=1000', '-e', 'USER_GID=1000',
      '-e', 'FORGEJO__security__INSTALL_LOCK=true',
      '-e', `FORGEJO__server__ROOT_URL=https://${stack.forgejoHost}/`,
      '-e', `FORGEJO__server__DOMAIN=${stack.forgejoHost}`,
      '-e', 'FORGEJO__database__DB_TYPE=sqlite3',
      '-e', 'FORGEJO__service__DISABLE_REGISTRATION=true',
      FORGEJO_IMAGE,
    ])
  }
  if (!running(stack.forgejoTls)) {
    shOk('docker', ['rm', '-f', stack.forgejoTls])
    sh('docker', [
      'run', '-d', '--name', stack.forgejoTls,
      '--network', FLEET_NETWORK, '--network-alias', stack.forgejoHost,
      '-v', `${stack.state}:/tls:ro`,
      '-v', `${join(stack.state, 'nginx.conf')}:/etc/nginx/nginx.conf:ro`,
      TLS_IMAGE,
    ])
  }
  for (let i = 0; i < 60; i++) {
    const up = await fetch(`${stack.forgejoRest}/api/v1/version`).then((r) => r.ok).catch(() => false)
    if (up) return
    await sleep(1000)
  }
  die('forgejo never answered')
}

function mintToken(stack, scopes) {
  return sh('docker', [
    'exec', '-u', '1000', stack.forgejoApp, 'forgejo', 'admin', 'user', 'generate-access-token',
    '--username', REPO_OWNER, '--token-name', `fixture-${Date.now()}`, '--scopes', scopes, '--raw',
  ])
    .split('\n')
    .pop()
}

/** Forgejo's token is shown once, so it is minted only when the stack holds none. */
async function ensureForgejoToken(stack, api) {
  const secrets = (await api('GET', '/api/admin/workspace-secrets')).secrets ?? []
  if (secrets.some((s) => s.name === SECRET_NAME)) return null
  sh('docker', [
    'exec', '-u', '1000', stack.forgejoApp, 'forgejo', 'admin', 'user', 'create',
    '--username', REPO_OWNER, '--password', randomBytes(16).toString('hex'),
    '--email', 'fixture@example.invalid', '--admin', '--must-change-password=false',
  ])
  const token = mintToken(stack, 'all')
  await api('POST', '/api/admin/workspace-secrets', {
    action: 'create',
    name: SECRET_NAME,
    title: SECRET_NAME,
    entries: [{ key: 'token', label: 'token', value: token }],
    allowedHosts: [stack.forgejoHost],
  })
  return token
}

async function ensureRepo(stack, token) {
  if (!token) return
  await forgejoFetch(stack, token, 'POST', '/user/repos', {
    name: REPO_NAME,
    auto_init: true,
    default_branch: 'main',
  })
  await forgejoFetch(stack, token, 'POST', `/repos/${REPO}/contents/calc.py`, {
    content: Buffer.from('def add(a, b):\n    return a + b\n').toString('base64'),
    message: 'seed calc.py',
    branch: 'main',
  })
}

async function ensureHost(stack, api, agent) {
  const { hosts = [] } = await api('GET', '/api/workbench/git-hosts')
  if (!hosts.some((h) => h.id === HOST_ID)) {
    await api('POST', '/api/workbench/git-hosts', {
      id: HOST_ID,
      kind: 'forgejo',
      name: 'Fixture Forgejo',
      baseUrl: `https://${stack.forgejoHost}`,
      apiUrl: `${stack.forgejoRest}/api/v1`,
      secret: SECRET_NAME,
    })
  }
  const test = await api('POST', `/api/workbench/git-hosts/${HOST_ID}/test`)
  if (!test.repos?.includes(REPO)) die(`the git host does not reach ${REPO}: ${JSON.stringify(test)}`)
  await api('PUT', `/api/workbench/repos/${agent.id}`, { repos: [{ host: HOST_ID, repo: REPO }] })
}

/** A model the endpoint no longer offers is why a fixture dies quietly: free ids rotate. */
async function ensureModel(api, agent, model) {
  const { endpoints } = await api('GET', '/api/fleet/endpoints')
  const endpoint = endpoints.find((e) => e.name === ENDPOINT)
  if (!endpoint) die(`no "${ENDPOINT}" endpoint on this stack`)
  const offered = (await api('GET', `/api/fleet/endpoints/${endpoint.id}/available`)).models ?? []
  if (!offered.includes(model)) {
    const free = offered.filter((m) => m.endsWith(':free') || m.startsWith('stealth/'))
    die(`${ENDPOINT} no longer offers ${model}. Free candidates: ${free.join(', ')}. Pass one with --model.`)
  }
  if (!endpoint.models.includes(model)) {
    await api('PUT', `/api/fleet/endpoints/${endpoint.id}`, { models: [...endpoint.models, model] })
  }
  for (const role of CODE_ROLES) {
    await api('PUT', '/api/admin/model-roles', { role, model: `${ENDPOINT}/${model}` })
  }
  await api('PATCH', `/api/fleet/defs/${agent.id}`, { developer: true })
  const { defs } = await api('GET', '/api/fleet/defs')
  const def = defs.find((d) => d.id === agent.id)
  await api('POST', `/api/fleet/defs/${agent.id}/edit`, {
    soul: def.latest.soul,
    main: { endpoint: ENDPOINT, model },
    aliases: [],
    fallbacks: [],
    note: 'rewind fixture model',
    apply: false,
  })
}

/** The roles reach a container as env and a rendered models.yml, so a role set
 *  after the last roll leaves the harness on the old model. */
async function rollAgent(stack, api, agent, model) {
  const onModel = (name) => shOk('docker', ['exec', name, 'printenv', 'PI_SMOL_MODEL'])?.endsWith(model)
  const before = agentContainer(stack)
  if (before && onModel(before)) return before
  await api('POST', '/api/fleet/render', {})
  await api('POST', `/api/fleet/agents/${agent.id}/control`, { action: before ? 'roll' : 'up' })
  for (let i = 0; i < 90; i++) {
    await sleep(2000)
    const name = agentContainer(stack)
    if (name && name !== before && onModel(name)) return name
  }
  die(`the agent never came up healthy on ${model}`)
}

/** Lives on the agent's volume, which a roll keeps. Hermes terminals run with
 *  HOME=/opt/data/home and the gateway's own processes with /opt/data. */
function trustForgejo(stack, container) {
  const pem = '/opt/data/rewind-fixture-ca.pem'
  sh('docker', ['exec', '-i', '-u', 'hermes', container, 'sh', '-c', `cat > ${pem}`], {
    input: readFileSync(join(stack.state, 'tls.crt')),
  })
  for (const home of ['/opt/data/home', '/opt/data']) {
    sh('docker', [
      'exec', '-u', 'hermes', '-e', `HOME=${home}`, container,
      'git', 'config', '--global', `http.https://${stack.forgejoHost}/.sslCAInfo`, pem,
    ])
  }
  const refs = sh('docker', [
    'exec', '-u', 'hermes', '-e', 'HOME=/opt/data/home', container,
    'git', 'ls-remote', `https://${stack.forgejoHost}/${REPO}.git`, 'refs/heads/main',
  ])
  if (!refs) die('the agent cannot read the fixture repo over TLS')
}

/** The request shape a real turn sends: tools and no `max_tokens`. */
async function preflight(stack, container, model) {
  const key = sh('docker', ['exec', container, 'printenv', 'LLM_API_KEY'])
  const res = await fetch(`${stack.app}/api/llm/v1/chat/completions`, {
    method: 'POST',
    headers: { authorization: `Bearer ${key}`, 'content-type': 'application/json' },
    body: JSON.stringify({
      model,
      messages: [{ role: 'user', content: 'Call the ping tool once.' }],
      tools: [
        {
          type: 'function',
          function: { name: 'ping', description: 'ping', parameters: { type: 'object', properties: {} } },
        },
      ],
    }),
  })
  if (!res.ok) {
    const body = await res.text()
    const why = res.status === 402 ? ' The account is out of credit for this model: top it up or use a free one.' : ''
    die(`gateway refused ${model}: ${res.status} ${body.slice(0, 200)}.${why}`)
  }
}

async function ensureBoard(stack, api, agent) {
  const { boards } = await api('GET', '/api/boards')
  const board =
    boards.find((b) => b.name === BOARD_NAME) ?? (await api('POST', '/api/boards', { name: BOARD_NAME })).board
  await api('PUT', `/api/boards/${board.id}/agents`, { add: [agent.model] })
  return board
}

async function setup(stack, slug, model) {
  const api = apiClient(stack, mintSession(stack))
  const agent = agentRow(stack, slug)
  say(`forgejo on ${stack.forgejoHost}`)
  await ensureForgejo(stack)
  await ensureRepo(stack, await ensureForgejoToken(stack, api))
  await ensureHost(stack, api, agent)
  say(`model ${model} on ${agent.model}`)
  await ensureModel(api, agent, model)
  const container = await rollAgent(stack, api, agent, model)
  trustForgejo(stack, container)
  await preflight(stack, container, model)
  const board = await ensureBoard(stack, api, agent)
  say('ready')
  process.stdout.write(`${JSON.stringify({ stack: stack.name, agent: agent.model, model, container, board: board.id })}\n`)
}

function toolSteps(transcript) {
  const counts = {}
  for (const line of transcript.split('\n')) {
    if (!line.startsWith('{"t":"toolfull"')) continue
    const frame = JSON.parse(line)
    if (frame.s !== 'running') counts[frame.v] = (counts[frame.v] ?? 0) + 1
  }
  return counts
}

async function report(stack, api, task, readToken) {
  const run = JSON.parse(
    psql(
      stack,
      `select row_to_json(r) from (select id, state, phase, error, result, checkpoint->>'turn' as turn from runs where subject_id = '${task.id}' order by created_at desc limit 1) r`,
    ),
  )
  const job = JSON.parse(
    psql(
      stack,
      `select row_to_json(j) from (select id, status, branch, pr_url from workbench_jobs where task_id = '${task.id}' order by created_at desc limit 1) j`,
    ) || 'null',
  )
  const status = psql(stack, `select status from tasks where id = '${task.id}'`)
  const { statuses } = await api('GET', `/api/boards/${task.boardId}/statuses`)
  const category = statuses.find((s) => s.key === status)?.category ?? null
  const tools = toolSteps((await api('GET', `/api/runs/${run.id}/transcript`)).body ?? '')
  let files = []
  if (job?.branch) {
    const compare = await forgejoFetch(stack, readToken, 'GET', `/repos/${REPO}/compare/main...${encodeURIComponent(job.branch)}`)
    files = (compare.json?.files ?? []).map((f) => f.filename)
  }
  return { run, job, ticket: { id: task.id, ref: task.ticketRef, status, category }, tools, files }
}

async function ticket(stack, slug, wait) {
  const api = apiClient(stack, mintSession(stack))
  const agent = agentRow(stack, slug)
  const board = await ensureBoard(stack, api, agent)
  const { task } = await api('POST', `/api/boards/${board.id}/tasks`, { ...TICKET, assignees: [agent.model] })
  say(`filed ${task.ticketRef} (${task.id})`)
  if (!wait) {
    process.stdout.write(`${JSON.stringify({ taskId: task.id, ticketRef: task.ticketRef, boardId: board.id })}\n`)
    return
  }
  const deadline = Date.now() + SESSION_DEADLINE_MS
  let last = ''
  while (Date.now() < deadline) {
    await sleep(5000)
    const line = psql(stack, `select state || ' | ' || phase from runs where subject_id = '${task.id}' order by created_at desc limit 1`)
    if (line !== last) say((last = line))
    if (/^(done|error|cancelled)\b/.test(line)) break
  }
  const out = await report(stack, api, task, mintToken(stack, 'read:repository'))
  process.stdout.write(`${JSON.stringify(out)}\n`)
  const stepCount = Object.values(out.tools).reduce((a, b) => a + b, 0)
  const healthy =
    out.run.state === 'done' && out.ticket.category === 'review' && stepCount >= 4 && out.files.length >= 2
  if (!healthy) die(`the session did not finish healthy: run ${out.run.state}, ticket ${out.ticket.status}`)
}

function teardown(stack) {
  for (const name of [stack.forgejoTls, stack.forgejoApp]) shOk('docker', ['rm', '-f', '-v', name])
  say('forgejo removed; the stack itself goes with the cleanup skill')
}

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    stack: { type: 'string' },
    agent: { type: 'string', default: 'atlas' },
    model: { type: 'string', default: DEFAULT_MODEL },
    'no-wait': { type: 'boolean', default: false },
  },
})
const [command] = positionals
if (!values.stack || !['setup', 'ticket', 'teardown'].includes(command)) {
  die('usage: rewind-fixture.mjs setup|ticket|teardown --stack <checkout> [--agent atlas] [--model id] [--no-wait]')
}
const stack = readStack(resolve(values.stack))
if (command === 'setup') await setup(stack, values.agent, values.model)
if (command === 'ticket') await ticket(stack, values.agent, !values['no-wait'])
if (command === 'teardown') teardown(stack)
