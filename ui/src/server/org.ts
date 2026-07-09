// The organization's identity — the business every agent works for. Stored in
// app_settings; woven automatically into muse generation (new souls anchor to
// the team) and into every RENDERED SOUL.md (existing agents too), so no agent
// ever introduces itself as working for the underlying platform.
import { getSetting, setSetting } from './audit'

export interface OrgProfile {
  /** The business name, e.g. "Outcrop Labs". */
  name: string
  /** One or two sentences on what the business does. */
  about: string
}

export async function orgProfile(): Promise<OrgProfile> {
  return {
    name: await getSetting('org_name', ''),
    about: await getSetting('org_about', ''),
  }
}

export async function setOrgProfile(patch: { name?: string; about?: string }): Promise<void> {
  if (patch.name !== undefined) await setSetting('org_name', patch.name.trim())
  if (patch.about !== undefined) await setSetting('org_about', patch.about.trim())
}

/** One prompt-ready sentence, or null when the org isn't configured yet. */
export function orgLine(p: OrgProfile): string | null {
  if (!p.name) return null
  return p.about ? `${p.name} — ${p.about}` : p.name
}

/** The header prepended to every rendered SOUL.md (a render-time projection —
 *  the stored soul stays clean and the header updates when the org does). */
export function orgSoulHeader(p: OrgProfile): string | null {
  const line = orgLine(p)
  if (!line) return null
  return (
    `<!-- organization context, rendered by Talaria -->\n` +
    `You are a member of ${p.name}'s team. ${p.about ? `${p.name}: ${p.about}. ` : ''}` +
    `When you introduce yourself or describe your role, you belong to ${p.name} — never to an underlying platform, framework, or model vendor.\n` +
    `Speak in product terms with teammates: say what you did and where they can find it in the workspace (Artifacts, boards, documents), ` +
    `not file paths, containers, or other internal plumbing — unless the person asks for technical detail or is clearly working at that level with you.`
  )
}
