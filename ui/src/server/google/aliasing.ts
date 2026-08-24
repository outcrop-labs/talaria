// How a Talaria org's people and agents are ADDRESSED on Google — pure string
// logic, no db and no fetch, so every rule here is directly unit-testable and
// cheap to call from wherever it lands (the login gate, provisioning's share
// rules, the confirm-send path).
//
// The one idea: the org's Google account is the workspace's single mailbox and
// identity. Humans log in with accounts at its domain; agents send from
// plus-addresses OF it (org+triage@domain) — mail to those addresses arrives
// in the org inbox the fleet already reads, and Gmail accepts them as From
// with no Google-side alias setup at all.

/** The domain of an email address, lowercased. null when there is no usable
 *  one (no @, nothing before it, nothing after it, null input). */
export function emailDomainOf(email: string | null | undefined): string | null {
  if (!email) return null
  const at = email.lastIndexOf('@')
  if (at < 1 || at === email.length - 1) return null
  return email.slice(at + 1).trim().toLowerCase()
}

/** A slug (or any label) folded into a plus-address tag: lowercase, [a-z0-9-],
 *  single dashes, nothing else. "Field Ops!" and "field-ops" address the same
 *  agent, and the tag can never carry characters Gmail would reject. */
export function plusTag(slug: string): string {
  return slug
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}

/** The org account's plus-address for a tag: jon@x.com + triage →
 *  jon+triage@x.com. An org email that is ITSELF plus-addressed (jon+old@x)
 *  has its tag replaced, not stacked. null when the org email has no usable
 *  local@domain shape. */
export function plusAddress(orgEmail: string, tag: string): string | null {
  const at = orgEmail.lastIndexOf('@')
  if (at < 1 || at === orgEmail.length - 1) return null
  const local = orgEmail.slice(0, at).replace(/\+.*$/, '')
  const domain = orgEmail.slice(at + 1)
  const clean = plusTag(tag)
  if (!local || !domain || !clean) return null
  return `${local}+${clean}@${domain}`
}

/** The address an ORG agent sends from: its stored override, else its derived
 *  plus-address, else null (the caller falls back to the org sendAs target /
 *  the account's own address). Never called for personal assistants — they
 *  send as their owner's account, where no alias applies. */
export function agentFromAddress(
  agent: { slug: string; emailAlias?: string | null },
  orgEmail: string | null | undefined,
): string | null {
  const override = agent.emailAlias?.trim()
  if (override) return override
  if (!orgEmail) return null
  return plusAddress(orgEmail, agent.slug)
}
