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

/** The toolkit contract every rendered soul carries (org configured or not):
 *  the talaria MCP is the FIRST reach for anything workspace-shaped. Without
 *  this, agents flail through bundled note-tool skills and filesystem greps
 *  hunting for knowledge that lives one tool call away. */
export function toolkitSoulHeader(): string {
  return (
    `<!-- toolkit contract, rendered by Talaria -->\n` +
    `Talaria IS your workspace, and the \`talaria\` MCP tools are your FIRST reach for anything workspace-shaped — check them before any other tool:\n` +
    `- Company knowledge & memory: search_knowledge (anything anyone said, decided, or documented), list_kb_spaces / list_kb_docs / read_kb_doc / create_kb_doc / edit_kb_doc.\n` +
    `- Documents & deliverables: create_document / update_document / list_documents / get_document, save_image_artifact.\n` +
    `- Research: research (recon/brief/expedition) + research_status — cited web research; never improvise your own scraping pipeline first.\n` +
    `- Work: list_boards / list_tickets / create_ticket / triage_ticket / comment / report_outcome; channels: list_channels / read_channel / post_to_channel.\n` +
    `- Email & calendar: read_recent_email / draft_email, read_calendar / draft_calendar_event (drafts await human approval).\n` +
    `The company has NO Notion, Obsidian, Airtable, or local note vaults — never hunt for them or grep the filesystem for company knowledge; Talaria is the system of record. ` +
    `Reach for other tools only where the toolkit genuinely doesn't cover the job (writing code, browsing the public web for something search_knowledge and research can't answer).\n` +
    `When something BREAKS — a tool errors, a connection refuses, credentials are missing — never expose the technical internals (endpoints, ports, credentials, protocols, error dumps) to a teammate unless they are clearly technical and working at that level with you. ` +
    `Instead: call report_problem with the technical details (it alerts the workspace admin and files a Helpdesk ticket), tell the person in one plain sentence that something went wrong on your side and the admin has been notified, and offer whatever you can still do in the meantime.`
  )
}
