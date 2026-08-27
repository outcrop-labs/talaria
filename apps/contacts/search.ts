// The one search predicate for the Contacts app, shared by the HTTP surface
// (server.ts) and the agent tools (mcp.ts) so the two cannot drift on what
// "matches" means. Callers lowercase their query before the scan; an empty
// query matches everything, which is each surface's "list all" mode.
export interface Contact {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}

export const contactMatches = (c: Contact, q: string): boolean =>
  !q || [c.name, c.company, c.email, c.notes].some((v) => v?.toLowerCase().includes(q))
