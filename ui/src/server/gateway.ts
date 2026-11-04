// The fleet roster, read off the manifest Talaria renders (fleet/fleet.json:
// { model, url, key } per agent and per model tier) — which ids are live
// agents, for the harness transport rule (harness/transport.ts
// `transportKind`).
//
// THIS PROCESS NO LONGER TALKS TO AN AGENT. A persona turn an app's harness
// runs goes to the Rust relay on the app's gateway key, and the relay drives
// it through its own persona door (talaria-gateway `proxy_chat`) with the
// turn's attribution recorded, so the agent's calls are metered once. The
// direct proxy that lived here — which held the agent's container key and
// left the agent's calls unattributed beside a row of its own — is gone.
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { FLEET_DIR } from './fleet-dir'

export interface AgentModel {
  id: string
  /** Display label — the leading segment of the id, capitalised. */
  label: string
  /** Role remainder of the id. */
  role: string
}

interface ManifestEntry {
  model: string
  url: string
  key: string
}

/** Read the rendered fleet manifest. Empty if the fleet hasn't been rendered. */
async function readManifest(): Promise<ManifestEntry[]> {
  const raw = await readFile(join(FLEET_DIR(), 'fleet.json'), 'utf8').catch(() => '')
  if (!raw) return []
  try {
    return JSON.parse(raw) as ManifestEntry[]
  } catch {
    return []
  }
}

/** Split an agent id into its display halves: "<slug>-<role>". */
export function describeAgent(id: string): AgentModel {
  const [first, ...rest] = id.split('-')
  const label = first ? first.charAt(0).toUpperCase() + first.slice(1) : id
  return { id, label, role: rest.join(' ') }
}

/** The fleet from the manifest — base models only (tier entries like
 *  "<base>-<alias>" are hidden from the picker).
 *
 *  An unrendered or missing manifest means an empty fleet, and says so. This
 *  used to fall back to three invented agents, which put names nobody created
 *  in front of the user on a fresh install and — worse — made "no agents" and
 *  "three agents" indistinguishable to every caller downstream. */
export async function listAgents(): Promise<AgentModel[]> {
  const manifest = await readManifest()
  if (!manifest.length) return []
  const ids = new Set(manifest.map((m) => m.model))
  const isTier = (id: string) => {
    const i = id.lastIndexOf('-')
    return i > 0 && ids.has(id.slice(0, i))
  }
  const bases = [...new Set(manifest.map((m) => m.model).filter((id) => !isTier(id)))]
  return bases.map(describeAgent)
}
