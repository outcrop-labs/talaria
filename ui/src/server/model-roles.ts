// Model Roles — which model handles each CLASS of activity. Customers tailor
// their stack in depth: the search model behind research, the utility model
// behind background chores (catalog blurbs, chat distills), and — as those
// surfaces land — vision, image generation, embeddings, and reranking.
//
// Resolution contract: an assignment only wins while it still ROUTES on the
// gateway; otherwise callers fall back to their own heuristics (env default →
// pl-main → first routable, sonar preference scan, ), so a deleted model can
// never silently break a subsystem. Unset = auto.
//
// AUDIT 1.6: routable is not the same as FIT. Routability was the only check
// here, so an admin could point `research-recon` at a model with no web search,
// `planSearch` would hand it to the search stages, and the run would come
// back a confident, uncited, hallucinated brief with nothing anywhere reporting
// a problem. Each role now DECLARES the capabilities its work needs, and
// `roleAssignmentIssues` reports the assignments a model is known not to be able
// to serve. It reports; it does not drop. Silently ignoring an admin's explicit
// pick would trade one invisible failure for another, and the admin may well
// know something the probe suite does not.
import { getSetting, setSetting } from './audit'
import { resolveRoute, routingFor } from './llm-gateway'
import { capabilityKey, missingCapabilities, type Capability } from './harness/capability'

export type ModelRole =
  | 'research-recon'
  | 'research-brief'
  | 'research-expedition'
  | 'utility'
  | 'code-light'
  | 'code-standard'
  | 'code-heavy'
  | 'vision'
  | 'image-generation'
  | 'embedding'
  | 'reranker'

export const MODEL_ROLES: Array<{
  role: ModelRole
  label: string
  hint: string
  /** False = the slot is reserved for a surface that hasn't landed yet. */
  wired: boolean
  /** What this role's WORK needs from a model (audit 1.6). Empty means the role
   *  genuinely runs on anything, which is a claim, not a shrug — see `utility`.
   *  Declared for reserved slots too, so the check is already right on the day
   *  the surface lands rather than something to remember then. */
  requires: Capability[]
}> = [
  {
    role: 'research-recon',
    label: 'Research · Recon',
    hint: 'Search stage for quick Recon passes. Needs a web-search-capable model. Auto: sonar.',
    wired: true,
    // The research pipeline's search stages are the whole point of these three
    // roles: a model without live search answers them from memory, in the same
    // confident shape, and the citations come out invented.
    requires: ['search'],
  },
  {
    role: 'research-brief',
    label: 'Research · Brief',
    hint: 'Search stages behind Brief documents. Auto: sonar-pro.',
    wired: true,
    requires: ['search'],
  },
  {
    role: 'research-expedition',
    label: 'Research · Expedition',
    hint: 'Search stages for deep Expedition runs. Auto: sonar-pro. Assigning a deep-research-class model (e.g. sonar-deep-research) makes each stage a full sweep: the engine runs fewer, bigger queries.',
    wired: true,
    requires: ['search'],
  },
  {
    role: 'utility',
    label: 'Utility',
    hint: 'Background chores: catalog blurbs, chat distills, summaries, Muse fallback. A fast, cheap model is ideal. Auto: env default → pl-main → first routable.',
    wired: true,
    // Deliberately empty. Utility is the LAST link in nearly every fallback
    // chain in the codebase, so a requirement here would strand titles, blurbs
    // and distills on a small self-host — exactly the install this audit is for.
    // Per-harness floors already refuse the handful of utility jobs that need
    // more (`defineHarness` floor.capabilities), which is the right altitude:
    // one harness declines, the rest keep working.
    requires: [],
  },
  {
    role: 'code-light',
    label: 'Workbench · Light effort',
    hint: 'Coding-harness runs for quick fixes and mechanical changes: agents pick the effort, this picks the model. A fast, cheap coder is ideal. Auto: utility chain.',
    wired: true,
    // `tools` is the load-bearing one and it is not a quality bar: the model
    // named here drives a CLI coding harness that reads and edits files through
    // tool calls, so without tool calling the run does not degrade, it does
    // nothing while reporting that it worked. `code` is the quality half. Both
    // apply at every effort — light effort is a smaller job, not a lesser
    // harness.
    requires: ['code', 'tools'],
  },
  {
    role: 'code-standard',
    label: 'Workbench · Standard effort',
    hint: 'The default coding-harness model for regular feature work. Auto: the light model, else the utility chain.',
    wired: true,
    requires: ['code', 'tools'],
  },
  {
    role: 'code-heavy',
    label: 'Workbench · Heavy effort',
    hint: 'The strongest coder for hard, cross-cutting work, used sparingly by design. Auto: the standard model.',
    wired: true,
    requires: ['code', 'tools'],
  },
  {
    role: 'vision',
    label: 'Image understanding',
    hint: 'Reserved: image inference for surfaces that analyze uploads without an agent persona.',
    wired: false,
    requires: ['vision'],
  },
  {
    role: 'image-generation',
    label: 'Image generation',
    hint: 'Reserved: native image generation when a creative surface lands.',
    wired: false,
    // Empty because it is HONEST, not because nothing is needed: `Capability`
    // has no member for image OUTPUT (`vision` is image understanding, the
    // opposite direction), and borrowing it would warn about the wrong thing on
    // every assignment. Fill this in when the union gains the member.
    requires: [],
  },
  {
    role: 'embedding',
    label: 'Embeddings',
    hint: 'RAG embeddings run NATIVE on the self-hosted TEI service (TALARIA_EMBED_MODEL in compose; swap = reindex). This slot takes over if gateway-served embedding models ever land.',
    wired: false,
    // Same honesty: embedding and reranking are not chat capabilities and the
    // union does not describe them. Today both run outside the gateway anyway.
    requires: [],
  },
  {
    role: 'reranker',
    label: 'Reranker',
    hint: 'Rerank providers (self-hosted TEI, Voyage, Together, NVIDIA, Pinecone, ) are configured in Admin → Retrieval: provider APIs, not gateway models.',
    wired: false,
    requires: [],
  },
]

const KEY = 'model_roles'

export async function getModelRoles(): Promise<Partial<Record<ModelRole, string>>> {
  return getSetting<Partial<Record<ModelRole, string>>>(KEY, {})
}

export async function setModelRole(role: ModelRole, model: string | null): Promise<void> {
  const cur = await getModelRoles()
  if (model) cur[role] = model
  else delete cur[role]
  await setSetting(KEY, cur)
}

/** The explicitly assigned model for a role — but only while it still routes.
 *  Null means "auto": the caller applies its own fallback heuristic.
 *
 *  AUDIT 1.6 asked whether this should also drop an assignment the model is
 *  unfit for. It must NOT. An admin's explicit pick disappearing into the auto
 *  chain is a second invisible failure, not a cure for the first, and probe
 *  facts are evidence rather than truth. Fitness surfaces two other ways
 *  instead, both of them visible: `roleAssignmentIssues` warns the admin at the
 *  moment of assignment, and each harness's own `floor` refuses at run time
 *  where a wrong answer would actually move a ticket. */
export async function resolveRoleModel(role: ModelRole): Promise<string | null> {
  const assigned = (await getModelRoles())[role]
  if (!assigned) return null
  return (await resolveRoute(assigned)) ? assigned : null
}

// ── Fitness (audit 1.6) ──────────────────────────────────────────────────────

/** Plain words for what the admin loses, one clause per capability, written to
 *  slot after the model id: "gpt-4o-mini has no web search, so …".
 *
 *  Partial on purpose. A capability no role requires needs no sentence, and the
 *  fallback below stays truthful for one added later — a stale, confidently
 *  wrong sentence would be worse than a plain one. */
const CONSEQUENCE: Partial<Record<Capability, string>> = {
  search: 'has no web search, so research runs will answer from memory and the citations will be invented',
  tools: 'cannot call tools, so a coding run cannot read or edit a single file',
  code: 'is not a coder, so its patches will need more repair than they save',
  vision: 'cannot read images, so anything sent to this slot comes back described from nothing',
}

const consequenceOf = (cap: Capability): string => CONSEQUENCE[cap] ?? `is known not to support ${cap}`

/** Capabilities the assigned model is KNOWN to lack for this role. Empty when
 *  the role requires nothing, when nothing routes the model, or — the important
 *  one — when nobody has measured it. UNKNOWN IS NOT A LACK; `capability.ts`
 *  owns that rule and it holds here for the same reason: a fresh self-host has
 *  probed nothing, and an admin page that warned about every model would teach
 *  people to ignore it.
 *
 *  Answered UNANIMOUSLY over the routing pool, exactly as `runHarness` does it.
 *  A bare model name can be served by several endpoints and capability is a
 *  property of the endpoint, so a capability counts as missing only when EVERY
 *  member says missing. `routingFor` rather than `resolveRoute`, because asking
 *  a question must not advance the round-robin cursor that live traffic reads. */
export async function roleModelGaps(role: ModelRole, model: string): Promise<Capability[]> {
  const required = MODEL_ROLES.find((r) => r.role === role)?.requires ?? []
  if (required.length === 0) return []
  try {
    const { endpoints, upstreamModel } = await routingFor(model)
    if (endpoints.length === 0) return [] // unroutable: `resolveRoleModel` already declines it
    let missing: Capability[] = [...required]
    for (const ep of endpoints) {
      const here = new Set(await missingCapabilities(capabilityKey(ep.name, upstreamModel), required))
      missing = missing.filter((cap) => here.has(cap))
      if (missing.length === 0) break
    }
    return missing
  } catch {
    // Advisory, never load-bearing. A settings row or endpoint list that fails
    // to read means we know nothing, and knowing nothing is not evidence.
    return []
  }
}

export interface RoleAssignmentIssue {
  role: ModelRole
  /** The assigned model id, spelled as the admin picked it. */
  model: string
  /** Never empty — a role with no known gap produces no issue at all. */
  missing: Capability[]
  /** One sentence for the admin UI, naming the capability in plain words. */
  note: string
}

/** Every role whose assigned model is known not to be able to do the work.
 *  Empty is the normal answer, including on an install that has probed nothing.
 *
 *  Reserved (`wired: false`) roles are included. Their surfaces do not exist
 *  yet, so nothing is broken today, but telling an admin now that their pick
 *  cannot see is strictly more useful than telling them the week the feature
 *  ships — and the row already carries a "reserved" chip saying it is inert. */
export async function roleAssignmentIssues(): Promise<RoleAssignmentIssue[]> {
  const assignments = await getModelRoles()
  // Concurrent because there are eleven roles and each gap check is two small
  // reads: serially that is an admin page load waiting out twenty-two
  // round trips for an answer that is almost always "no issues". `Promise.all`
  // preserves order, so the panel's rows stay in MODEL_ROLES order.
  const checked = await Promise.all(
    MODEL_ROLES.map(async (spec) => {
      // Unset = auto, and the auto chains already reason about fitness (the
      // sonar preference scan, the utility fall-down). Nothing to warn about.
      const model = assignments[spec.role]
      if (!model) return null
      const missing = await roleModelGaps(spec.role, model)
      if (missing.length === 0) return null
      const issue: RoleAssignmentIssue = {
        role: spec.role,
        model,
        missing,
        note: `${model} ${missing.map(consequenceOf).join(', and ')}. The assignment stands; set the role back to Auto if that is not what you meant.`,
      }
      return issue
    }),
  )
  return checked.filter((i): i is RoleAssignmentIssue => i !== null)
}
