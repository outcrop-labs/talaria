// GROUP DMs — a "New message" to several people and/or agents at once.
//
// A group DM is a `dm` channel with no single `peer`: the server lists its
// other `members` and seated `agents` instead (a two-person DM keeps its
// `peer`). These are the pure rules the rail, the header and the New message
// pane share: how one is labelled, how many others it holds, and which
// existing DM a set of recipients already has.

export interface DmLike {
  kind: string
  name: string
  peer?: { userId: string; name: string | null; email: string | null } | null
  members?: { userId: string; name: string | null; email: string | null }[]
  agents?: string[]
}

/** A DM that is not a plain two-person one. */
export const isGroupDm = (c: DmLike): boolean => c.kind === 'dm' && !c.peer

const firstName = (name: string | null, email: string | null): string =>
  (name?.trim() ? name.trim().split(/\s+/)[0] : (email ?? '').split('@')[0]) || 'someone'

/**
 * What a group DM is called: its name if it has one, else everyone else's
 * first names then its agents' labels — "Maya, Jordan, Atlas".
 */
export function groupDmLabel(c: DmLike, agentLabel: (model: string) => string): string {
  if (c.name.trim()) return c.name.trim()
  const people = (c.members ?? []).map((m) => firstName(m.name, m.email))
  const agents = (c.agents ?? []).map(agentLabel)
  return [...people, ...agents].join(', ') || 'Direct message'
}

/** How many others are in it — the count on the rail row's badge. */
export const groupDmCount = (c: DmLike): number => (c.members?.length ?? 0) + (c.agents?.length ?? 0)

/**
 * The DM these recipients already have, if any: the exact same people (you
 * aside) and the exact same agents. One person and no agents is their plain
 * DM.
 */
export function findDm<C extends DmLike & { id: string }>(
  channels: readonly C[],
  selfId: string | null,
  userIds: readonly string[],
  agents: readonly string[],
): C | null {
  const people = [...new Set(userIds.filter((u) => u !== selfId))].sort()
  const bots = [...new Set(agents)].sort()
  if (people.length === 0 && bots.length <= 1) return null
  const same = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((x, i) => x === b[i])
  for (const c of channels) {
    if (c.kind !== 'dm') continue
    if (c.peer) {
      if (bots.length === 0 && people.length === 1 && c.peer.userId === people[0]) return c
      continue
    }
    const theirPeople = (c.members ?? []).map((m) => m.userId).sort()
    const theirBots = [...(c.agents ?? [])].sort()
    if (same(theirPeople, people) && same(theirBots, bots)) return c
  }
  return null
}
