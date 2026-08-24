import type { WaitingRole, WaitingSlot } from './registry'

/**
 * Every place in the cockpit that waits on an agent, in one table.
 *
 * This is the "library" the rotation deals across, and keeping it as a table
 * rather than as props scattered over forty components buys three things:
 *
 *   1. The dealer can see all the sites at once, so it can hand out THIRTY
 *      DISTINCT states instead of hashing each site independently and landing
 *      the same mark in the chat turn and the tool row by coincidence.
 *   2. A reviewer can read the app's whole waiting behaviour in one screen.
 *   3. A site's role and slot live next to its siblings, which is where a
 *      wrong one is visible — `admin/storage-sync` sitting among `background`
 *      neighbours reads as correct in a way it does not at its call site.
 *
 * A call site names its key and nothing else. Adding a surface means adding a
 * row here; a `<Waiting>` naming a key with no row is a type error, not a
 * silent fallback, because that is a mark nobody has decided the meaning of.
 */

export interface WaitingSite {
  /** What the wait means — sets the tempo (spec §9 rungs). */
  role: WaitingRole
  /** Where it physically sits — constrains which states can be dealt. */
  slot: WaitingSlot
  /** What is being waited on. Read by nobody at runtime; read by everybody here. */
  note: string
}

/**
 * Keys are `surface/what`. They are the dealer's input, so they are stable
 * identifiers, not descriptions — renaming one re-deals that site.
 */
export const WAITING_SITES = {
  /* ── The generic block ────────────────────────────────────────────────── */
  'generating/block': {
    role: 'reasoning',
    slot: 'inline',
    note: "Fallback for a <Generating> that carries a label but hasn't claimed a site.",
  },

  /* ── Chat ─────────────────────────────────────────────────────────────── */
  'chat/first-token': {
    role: 'submitting',
    slot: 'inline',
    note: 'Assistant turn, nothing streamed yet — the gap this whole set exists for.',
  },
  'chat/reasoning': {
    role: 'reasoning',
    slot: 'inline',
    note: 'Assistant turn where tools/reasoning stream but no prose has landed.',
  },
  'chat/message-first-token': {
    role: 'submitting',
    slot: 'inline',
    note: 'Conversation message row awaiting its first token.',
  },
  'chat/tool': { role: 'tool', slot: 'inline', note: 'A tool call in flight, in the 11px mono row.' },
  'chat/attach': { role: 'submitting', slot: 'button', note: 'Uploading an attachment from the composer.' },
  'chat/relay': { role: 'submitting', slot: 'button', note: 'Relaying the thread to another agent.' },
  'chat/doc-resync': {
    role: 'reasoning',
    slot: 'inline',
    note: 'Rewriting the plan document from the conversation (overlay).',
  },
  'plan/draft': {
    role: 'reasoning',
    slot: 'inline',
    note: 'The plan modal while an agent reads the conversation and drafts tickets.',
  },

  /* ── Comms ────────────────────────────────────────────────────────────── */
  'comms/conclude': {
    role: 'reasoning',
    slot: 'inline',
    note: 'Summarising what a channel decided, then archiving it (overlay).',
  },

  /* ── Assistant ────────────────────────────────────────────────────────── */
  'assistant/wizard': { role: 'submitting', slot: 'button', note: 'Assistant wizard step submitting.' },

  /* ── Research ─────────────────────────────────────────────────────────── */
  'research/start': { role: 'submitting', slot: 'inline', note: 'Kicking off a research run from the composer.' },
  'research/run': {
    role: 'reasoning',
    slot: 'status',
    note: 'A research run queued or running — the one standalone status panel.',
  },

  /* ── Workflows ────────────────────────────────────────────────────────── */
  'workflows/muse-draft': { role: 'reasoning', slot: 'inline', note: 'Muse drafting a skill.' },

  /* ── Fleet ────────────────────────────────────────────────────────────── */
  'fleet/agent-controls': { role: 'submitting', slot: 'inline', note: 'An agent lifecycle control mid-flight.' },
  'fleet/agent-create': { role: 'submitting', slot: 'button', note: 'Create-agent modal submitting.' },
  'fleet/agent-design': { role: 'reasoning', slot: 'inline', note: 'Designing an agent: identity, soul, starter skills.' },
  'fleet/agent-hire': { role: 'tool', slot: 'inline', note: 'Rendering config, starting the container, waiting for health.' },
  'fleet/agent-apply': { role: 'tool', slot: 'inline', note: 'Rolling a new container up beside the old one.' },
  'fleet/cron-save': { role: 'submitting', slot: 'button', note: 'Cron form submitting.' },
  'fleet/cron-design': { role: 'reasoning', slot: 'inline', note: 'Designing a job: name, schedule, prompt.' },
  'fleet/federate': { role: 'submitting', slot: 'button', note: 'Federating with another instance.' },
  'fleet/mcp-roll': { role: 'tool', slot: 'inline', note: 'An MCP server rolling in the agent detail tab.' },
  'fleet/mcp-apply': { role: 'tool', slot: 'inline', note: 'Applying an MCP change across an agent.' },
  'fleet/version-revert': { role: 'tool', slot: 'inline', note: 'Reverting to an earlier agent version.' },

  /* ── MCP ──────────────────────────────────────────────────────────────── */
  'mcp/search': { role: 'tool', slot: 'inline', note: 'Marketplace search in flight.' },
  'mcp/install': { role: 'submitting', slot: 'button', note: 'Installing a server from the marketplace.' },
  'mcp/server-refresh': { role: 'tool', slot: 'button', note: 'Re-probing a server card for its tool list.' },

  /* ── The brief ────────────────────────────────────────────────────────── */
  'brief/writing': { role: 'reasoning', slot: 'inline', note: 'The daily brief being opened for this reader — lede first, then the day.' },

  /* ── Models ───────────────────────────────────────────────────────────── */
  'models/fitness-detail': { role: 'background', slot: 'inline', note: 'Fitness evaluation running, detail view.' },
  'models/fitness-panel': { role: 'background', slot: 'inline', note: 'Fitness evaluation running, settings panel.' },
  'models/endpoint-remove': { role: 'tool', slot: 'inline', note: 'Removing an endpoint across the fleet.' },

  /* ── Observability ────────────────────────────────────────────────────── */
  'observability/live-total': { role: 'background', slot: 'inline', note: 'Fleet-wide generating count, live monitor.' },
  'observability/live-agent': { role: 'background', slot: 'inline', note: 'Per-agent generating flag, live monitor.' },

  /* ── Admin ────────────────────────────────────────────────────────────── */
  'admin/retrieval-reindex': { role: 'background', slot: 'inline', note: 'Rebuilding collections or refilling from sources.' },
  'admin/retrieval-backfill': { role: 'background', slot: 'inline', note: 'Backfilling the retrieval index.' },
  'admin/storage-migrate': { role: 'background', slot: 'inline', note: 'Moving blobs between storage backends.' },
  'admin/storage-sync': { role: 'background', slot: 'inline', note: 'Syncing blobs to the replica.' },
  'admin/key-rotate': { role: 'background', slot: 'inline', note: 'Re-encrypting every secret under a new data key.' },
} as const satisfies Record<string, WaitingSite>

export type WaitingSiteKey = keyof typeof WAITING_SITES

/**
 * Deal order. Sorted for the same reason the registry is: the hand a seed
 * produces must not depend on where someone chose to type a new row.
 */
export const WAITING_SITE_KEYS: readonly WaitingSiteKey[] = (
  Object.keys(WAITING_SITES) as WaitingSiteKey[]
).sort()
