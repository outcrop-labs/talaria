// Platform sub-agents — Talaria's OWN workers, separate from the Hermes fleet.
// Each one is a named, model-agnostic harness for a single internal job
// (distilling chats, concluding relays, drafting with Muse, writing catalog
// blurbs, judging ticket outcomes), with its own prompt and skills tailored to
// that job. Which model powers each agent is configured GRANULARLY on
// Models → Platform; unset = the job's auto chain keeps working untouched.
//
// Resolution contract mirrors model roles: an assignment only wins while it
// still ROUTES on the gateway — a deleted model can never silently break a
// subsystem, the job just falls back to its own chain.
import { getSetting, setSetting } from './audit'
import { resolveRoute } from './llm-gateway'

export type PlatformAgentId = 'muse' | 'distiller' | 'concluder' | 'blurb-writer' | 'titler' | 'librarian' | 'judge' | 'briefer' | 'summarizer'

export const PLATFORM_AGENTS: Array<{
  id: PlatformAgentId
  label: string
  job: string
  /** What its harness brings to the job — shown as chips in the panel. */
  skills: string[]
  /** What "Auto" resolves to, in words. */
  auto: string
  /** False = the model is fixed by design (e.g. the user's own assistant). */
  assignable: boolean
}> = [
  {
    id: 'muse',
    label: 'Muse',
    job: 'The writing partner behind prompt-editing everywhere: documents, agent souls, and templates.',
    skills: ['org voice', 'register-aware drafting', 'template skeleton harness'],
    auto: "the requesting user's preferred model, else the Utility role chain",
    assignable: true,
  },
  {
    id: 'distiller',
    label: 'Distiller',
    job: 'Condenses idle agent chats into their durable substance before they archive — what feeds each user’s private brain.',
    skills: ['transcript distillation', 'decision & preference extraction'],
    auto: "the chat owner's muse (their preference, else the Utility role chain)",
    assignable: true,
  },
  {
    id: 'concluder',
    label: 'Concluder',
    job: 'Writes the closing summary when a relay concludes — decisions, deliverables, follow-ups.',
    skills: ['multi-party synthesis', 'action-item extraction'],
    auto: "the concluding user's muse (their preference, else the Utility role chain)",
    assignable: true,
  },
  {
    id: 'blurb-writer',
    label: 'Catalog writer',
    job: 'Keeps the model catalog human: one-line plain-language blurbs for every registered model.',
    skills: ['plain-language descriptions', 'org-profile awareness'],
    auto: 'the Utility role chain',
    assignable: true,
  },
  {
    id: 'titler',
    label: 'Titler',
    job: 'Names things as they take shape: chats and plans after their first exchange, research runs from their question.',
    skills: ['concise naming', 'never clobbers user-chosen names'],
    auto: 'the Utility role chain — a fast, cheap model is ideal',
    assignable: true,
  },
  {
    id: 'summarizer',
    label: 'Summarizer',
    job: 'Keeps the Studio readable: one plain line per skill saying what it teaches, regenerated only when the skill changes.',
    skills: ['one-line gist extraction', 'content-hash change detection'],
    auto: 'the Utility role chain — a fast, cheap model is ideal',
    assignable: true,
  },
  {
    id: 'librarian',
    label: 'Librarian',
    job: 'Maintains each knowledge space’s OKF digest — summaries and links of the promoted documents, regenerated as promotions change.',
    skills: ['knowledge digestion', 'summaries with links', 'autonomous upkeep'],
    auto: 'the Utility role chain',
    assignable: true,
  },
  {
    id: 'judge',
    label: 'Judge',
    job: 'Reviews agents’ reported ticket outcomes against the ask — verdicts and findings on boards with judging on.',
    skills: ['outcome verification', 'structured verdicts'],
    auto: 'pl-main when judging is enabled without a pick',
    assignable: true,
  },
  {
    id: 'briefer',
    label: 'Briefer',
    job: 'Writes your daily brief every morning and follows it through the day as it moves.',
    skills: ['scope-aware summarizing', 'the append-only document contract'],
    auto: 'always the user’s personal assistant — its persona and privacy are the point',
    assignable: false,
  },
]

const KEY = 'platform_agent_models'

/** Current raw assignments (id → model), unvalidated. */
export const getPlatformAgentModels = () => getSetting<Partial<Record<PlatformAgentId, string>>>(KEY, {})

export async function setPlatformAgentModel(id: PlatformAgentId, model: string | null): Promise<void> {
  const cur = await getPlatformAgentModels()
  if (model) cur[id] = model
  else delete cur[id]
  await setSetting(KEY, cur)
}

/** The admin-assigned model for a platform agent — but only while it still
 *  routes on the gateway. Null = unassigned or stale: use the job's auto chain. */
export async function platformAgentModel(id: PlatformAgentId): Promise<string | null> {
  const assigned = (await getPlatformAgentModels())[id]
  if (assigned && (await resolveRoute(assigned))) return assigned
  return null
}
