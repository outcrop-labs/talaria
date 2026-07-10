// Model Roles — which model handles each CLASS of activity. Customers tailor
// their stack in depth: the search model behind research, the utility model
// behind background chores (catalog blurbs, chat distills), and — as those
// surfaces land — vision, image generation, embeddings, and reranking.
//
// Resolution contract: an assignment only wins while it still ROUTES on the
// gateway; otherwise callers fall back to their own heuristics (env default →
// pl-main → first routable, sonar preference scan, …), so a deleted model can
// never silently break a subsystem. Unset = auto.
import { getSetting, setSetting } from './audit'
import { resolveRoute } from './llm-gateway'

export type ModelRole =
  | 'research-recon'
  | 'research-brief'
  | 'research-expedition'
  | 'utility'
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
}> = [
  {
    role: 'research-recon',
    label: 'Research · Recon',
    hint: 'Search stage for quick Recon passes. Needs a web-search-capable model. Auto: sonar.',
    wired: true,
  },
  {
    role: 'research-brief',
    label: 'Research · Brief',
    hint: 'Search stages behind Brief documents. Auto: sonar-pro.',
    wired: true,
  },
  {
    role: 'research-expedition',
    label: 'Research · Expedition',
    hint: 'Search stages for deep Expedition runs. Auto: sonar-pro. Assigning a deep-research-class model (e.g. sonar-deep-research) makes each stage a full sweep — the engine runs fewer, bigger queries.',
    wired: true,
  },
  {
    role: 'utility',
    label: 'Utility',
    hint: 'Background chores: catalog blurbs, chat distills, summaries, Muse fallback. A fast, cheap model is ideal. Auto: env default → pl-main → first routable.',
    wired: true,
  },
  {
    role: 'vision',
    label: 'Image understanding',
    hint: 'Reserved: image inference for surfaces that analyze uploads without an agent persona.',
    wired: false,
  },
  {
    role: 'image-generation',
    label: 'Image generation',
    hint: 'Reserved: native image generation when a creative surface lands.',
    wired: false,
  },
  {
    role: 'embedding',
    label: 'Embeddings',
    hint: 'Reserved: RAG embeddings currently run on the dedicated embeddings service (TALARIA_EMBED_URL); this slot takes over when gateway-served embedding models land.',
    wired: false,
  },
  {
    role: 'reranker',
    label: 'Reranker',
    hint: 'Reserved: reranking merged multi-collection RAG results (the retrieval-quality tail).',
    wired: false,
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
 *  Null means "auto": the caller applies its own fallback heuristic. */
export async function resolveRoleModel(role: ModelRole): Promise<string | null> {
  const assigned = (await getModelRoles())[role]
  if (!assigned) return null
  return (await resolveRoute(assigned)) ? assigned : null
}
