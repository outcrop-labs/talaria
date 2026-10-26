// THE APP HARNESS MODEL SETTINGS — the model an admin assigned to each
// harness an app ships, keyed by its slot `app:<slug>/<harnessId>`.
//
// One setting, `app_harness_models`, read by both resolvers: this file for the
// TS chain (harness/model.ts, the 'slot' step) and talaria-harness-model's
// `app_harness_model` for the Rust one. Admin → Models → Fitness writes it
// (`PUT /api/admin/app-harness-models`, Rust) on an app harness's column.
//
// Resolution contract, the same one platform agents and model roles keep: an
// assignment wins only while it still ROUTES on the gateway — a deleted model
// never silently breaks a harness, the chain just falls to its next step.
import { getSetting } from './audit'
import { resolveRoute } from './llm-gateway'

/** The settings key. Rust twin: talaria-harness-model `APP_HARNESS_MODELS_KEY`. */
export const APP_HARNESS_MODELS_KEY = 'app_harness_models'

/** The slot an app harness's model setting lives under. The host builds it
 *  from the app's slug and the harness id, never from anything the app says. */
export const appHarnessSlot = (slug: string, harnessId: string): string => `app:${slug}/${harnessId}`

/** Current raw assignments (slot → model), unvalidated. A stored value that
 *  is not an object of strings reads as no assignments. */
export async function getAppHarnessModels(): Promise<Record<string, string>> {
  const raw = await getSetting<unknown>(APP_HARNESS_MODELS_KEY, {})
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) if (typeof v === 'string' && v) out[k] = v
  return out
}

/** The admin-assigned model for an app harness slot — only while it routes.
 *  Null = unassigned or stale: the harness's own chain carries on. */
export async function appHarnessModel(slot: string): Promise<string | null> {
  const assigned = (await getAppHarnessModels())[slot]
  if (assigned && (await resolveRoute(assigned))) return assigned
  return null
}
