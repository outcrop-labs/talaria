// Fine-grained user permissions. Three layers, most specific wins:
//   1. per-user overrides        (user_permissions rows — allow or deny)
//   2. org-wide member defaults  (app_settings 'member_default_permissions')
//   3. the catalog's shipped defaults below
// Admins hold every permission unconditionally. Resource-level ACLs (board
// membership, KB editors, plan/research shares, personal-agent ownership)
// stay where they are — a permission says what a user CAN DO, an ACL says
// what they can do it TO.
import { json } from '@/server/http'
import { db } from './db/pg'
import { getSetting, setSetting } from './audit'
import { getSessionUser, type SessionUser } from './auth/session'

export type Perm =
  | 'agents.manage'
  | 'research.run'
  | 'plans.create'
  | 'comms.channels'
  | 'comms.relays'
  | 'boards.create'
  | 'kb.edit'
  | 'kb.official'
  | 'artifacts.create'
  | 'artifacts.publish'
  | 'files.upload'
  | 'templates.manage'
  | 'models.mint-keys'

export const PERMISSIONS: Array<{
  id: Perm
  label: string
  hint: string
  group: string
  /** What a plain member may do out of the box. */
  memberDefault: boolean
}> = [
  // ── Agents ──
  {
    id: 'agents.manage',
    label: 'Manage agents',
    hint: 'Hire, retire, and configure org agents: souls, skills, crons, start/stop. Agent secrets and infrastructure stay admin-only.',
    group: 'Agents',
    memberDefault: false,
  },
  // ── Work ──
  {
    id: 'research.run',
    label: 'Run research',
    hint: 'Start research runs (recon, briefs, expeditions).',
    group: 'Work',
    memberDefault: true,
  },
  {
    id: 'plans.create',
    label: 'Create plans',
    hint: 'Start plan conversations and their living documents.',
    group: 'Work',
    memberDefault: true,
  },
  {
    id: 'boards.create',
    label: 'Create boards',
    hint: 'Create new boards. Working on boards they belong to is membership, not this.',
    group: 'Work',
    memberDefault: true,
  },
  // ── Comms ──
  {
    id: 'comms.channels',
    label: 'Create channels',
    hint: 'Create persistent channels. Joining and posting is membership.',
    group: 'Comms',
    memberDefault: true,
  },
  {
    id: 'comms.relays',
    label: 'Start relays',
    hint: 'Spin up relays — ephemeral working groups that conclude and archive.',
    group: 'Comms',
    memberDefault: true,
  },
  // ── Content ──
  {
    id: 'kb.edit',
    label: 'Edit knowledge',
    hint: 'Create and edit knowledge docs (per-doc/space ACLs still apply).',
    group: 'Content',
    memberDefault: true,
  },
  {
    id: 'kb.official',
    label: 'Curate knowledge',
    hint: 'Create spaces and mark docs OFFICIAL — content that grounds every agent.',
    group: 'Content',
    memberDefault: false,
  },
  {
    id: 'artifacts.create',
    label: 'Create documents',
    hint: 'Create documents and artifacts.',
    group: 'Content',
    memberDefault: true,
  },
  {
    id: 'artifacts.publish',
    label: 'Publish to the web',
    hint: 'Make artifacts PUBLIC — reachable by anyone with the link, outside the org.',
    group: 'Content',
    memberDefault: false,
  },
  {
    id: 'files.upload',
    label: 'Upload files',
    hint: 'Attach files and images to chats, channels, and tickets.',
    group: 'Content',
    memberDefault: true,
  },
  {
    id: 'templates.manage',
    label: 'Manage templates',
    hint: 'Edit the org-wide ticket and plan templates everyone starts from.',
    group: 'Content',
    memberDefault: false,
  },
  // ── Models ──
  {
    id: 'models.mint-keys',
    label: 'Mint API keys',
    hint: 'Mint personal LLM-gateway API keys for external tools.',
    group: 'Models',
    memberDefault: false,
  },
]

const CATALOG_DEFAULTS = new Map(PERMISSIONS.map((p) => [p.id, p.memberDefault]))
const ORG_DEFAULTS_KEY = 'member_default_permissions'

/** Org-tuned member defaults (admin-editable), sparse over the catalog. */
export const getOrgDefaultPerms = () => getSetting<Partial<Record<Perm, boolean>>>(ORG_DEFAULTS_KEY, {})

export async function setOrgDefaultPerm(perm: Perm, enabled: boolean | null): Promise<void> {
  const cur = await getOrgDefaultPerms()
  if (enabled === null) delete cur[perm]
  else cur[perm] = enabled
  await setSetting(ORG_DEFAULTS_KEY, cur)
}

export async function getUserPermOverrides(userId: string): Promise<Partial<Record<Perm, boolean>>> {
  const sql = await db()
  const rows = (await sql`select perm, allowed from user_permissions where user_id = ${userId}`) as unknown as Array<{
    perm: Perm
    allowed: boolean
  }>
  return Object.fromEntries(rows.map((r) => [r.perm, r.allowed]))
}

export async function setUserPermOverride(userId: string, perm: Perm, allowed: boolean | null): Promise<void> {
  const sql = await db()
  if (allowed === null) {
    await sql`delete from user_permissions where user_id = ${userId} and perm = ${perm}`
  } else {
    await sql`
      insert into user_permissions (user_id, perm, allowed) values (${userId}, ${perm}, ${allowed})
      on conflict (user_id, perm) do update set allowed = ${allowed}
    `
  }
}

/** The user's effective permission set. Admins: everything. */
export async function userPermissions(userId: string, role: 'admin' | 'member'): Promise<Perm[]> {
  if (role === 'admin') return PERMISSIONS.map((p) => p.id)
  const [org, overrides] = await Promise.all([getOrgDefaultPerms(), getUserPermOverrides(userId)])
  return PERMISSIONS.filter((p) => overrides[p.id] ?? org[p.id] ?? CATALOG_DEFAULTS.get(p.id)).map((p) => p.id)
}

export async function hasPerm(user: { id: string; role: 'admin' | 'member' }, perm: Perm): Promise<boolean> {
  if (user.role === 'admin') return true
  const [org, overrides] = await Promise.all([getOrgDefaultPerms(), getUserPermOverrides(user.id)])
  return overrides[perm] ?? org[perm] ?? CATALOG_DEFAULTS.get(perm) ?? false
}

/** Route gate: session + permission in one call. Returns the user, or a
 *  ready-to-return 401/403 Response. Usage:
 *    const gate = await requirePerm(request, 'research.run')
 *    if (gate instanceof Response) return gate
 *    const user = gate */
export async function requirePerm(request: Request, perm: Perm): Promise<SessionUser | Response> {
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  if (!(await hasPerm(user, perm))) {
    return json({ error: `you don't have permission to do that (${perm})` }, { status: 403 })
  }
  return user
}
