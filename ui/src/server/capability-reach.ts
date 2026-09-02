// CAN THIS DEPLOYMENT DO IT — as opposed to: can this model do it.
//
// THE CATEGORY ERROR THIS FIXES, in the words of the bug report that found it:
// "search can be TOOL DRIVEN in Talaria, so saying a MODEL is not capable is
// true, but at the harness level models are slotting into, it's completely
// untrue."
//
// That is exactly right, and the fitness page was making the error out loud. It
// probed `deepseek/deepseek-v4-flash`, measured `tools` at 100% and
// `tool-select` at 100% over four prompts, measured `search` at 0% because the
// model does not browse, and then reported the model Not-a-fit for all three
// Research slots. Every number was correct. The conclusion was wrong, because
// the slot an admin assigns is not a bare model — it is a model running inside
// Talaria, with the tools this org has registered, behind a gateway that can
// hand it definitions and run a loop. A model that calls the right tool every
// time and has a search tool in front of it can do research. Telling an admin
// otherwise costs them a capable, cheap model for no reason.
//
// SO CAPABILITY GETS TWO QUESTIONS INSTEAD OF ONE:
//
//   capability.ts   does the MODEL do this natively? An attribute of weights and
//                   of the endpoint serving them. Probes measure it, catalogs
//                   declare it, and it is the honest answer to "what did we
//                   observe".
//   THIS FILE       can the RUN reach it? Natively, or through a tool the
//                   platform will supply. This is the question a floor should
//                   ask before refusing, and the question a slot verdict should
//                   answer, because it is the question the admin is actually
//                   asking when they pick a model from a dropdown.
//
// WHAT IT DOES NOT DO: invent reach. A tool path counts only when the tool is
// REALLY THERE — registered and enabled in this install, or supplied by Talaria
// itself and CHECKED to be working (see `capability-platform.ts`) — and the
// model can actually call tools. An org with neither gets the same "not a fit"
// it got before, with a materially better sentence naming the thing to go and
// install rather than blaming the model.
import { getSetting } from './audit'
import { listMcpServers, type McpServer } from './mcp-registry'
import { getCapabilities, type Capability, type CapabilityFact } from './harness/capability'
import { platformSupply } from './capability-platform'

/** How a capability is satisfied for one run. */
export type ReachVia = 'native' | 'tool'

export interface Reach {
  capability: Capability
  reached: boolean
  via: ReachVia | null
  /** The registered tool that supplies it, when `via` is 'tool'. */
  supplier: { server: string; tool: string } | null
  /** One sentence for the admin, written for the model picker rather than for
   *  a developer. Always populated — the negative case is the one that has to
   *  say what to do about it. */
  detail: string
}

// ── Which capabilities a tool can stand in for ───────────────────────────────

/** THE TABLE IS SHORT ON PURPOSE, and the discipline is the same one the Rust
 *  fitness scorer (api/src/fitness/score.rs) applies to `DECLARED_EDGES`: an
 *  entry earns its place by being a case where a tool genuinely does the job,
 *  not by being conceivable.
 *
 *  `search` qualifies completely. A web-search tool returns the same thing a
 *  sonar model returns — passages with source URLs — and the synthesis stage
 *  downstream cannot tell which produced them, because it consumes findings and
 *  a source list either way.
 *
 *  WHAT IS DELIBERATELY ABSENT, so the next author does not have to relitigate:
 *    `vision`      an OCR or captioning tool describes an image in words. That
 *                  is a different, lossier capability wearing the same name, and
 *                  recording it as vision would let a model be assigned to
 *                  Image understanding on the strength of alt text.
 *    `code`        the workbench RUNS code; the capability is about WRITING it.
 *                  A sandbox does not make a model a programmer.
 *    `long-context` chunking is a strategy, not a capability, and the harness
 *                  that needs the window is the one that would have to implement
 *                  it. Nothing here can supply it on that harness's behalf.
 *    `json`, `json-strict`, `tools`, `tool-select`, `instruction-following`
 *                  properties of the model's own decoding. There is no outside
 *                  thing to hand it. */
const TOOL_REACHABLE: ReadonlyArray<{
  capability: Capability
  /** Tool names that supply it, lowercased. Matched as a whole word against the
   *  tool name so `search_knowledge` (Talaria's own RAG over the org's docs, not
   *  the live web) cannot be mistaken for a web search tool. */
  names: readonly string[]
  /** Words in a tool's DESCRIPTION that corroborate a name match. */
  hints: readonly string[]
}> = [
  {
    capability: 'search',
    names: ['web_search', 'websearch', 'search_web', 'brave_web_search', 'tavily_search', 'exa_search', 'google_search', 'perplexity_search', 'serper_search'],
    hints: ['web', 'internet', 'live', 'browse', 'online'],
  },
  {
    // A MODEL THAT CANNOT SEE CAN STILL CALL A TOOL. Talaria's own
    // `describe_image` reads an image with whatever model the org assigned to
    // the `vision` role, so a deployment reaches `vision` even when the model
    // assigned to a slot does not have it — which is the difference between a
    // red cell and an honest `supplied` one.
    capability: 'vision',
    names: ['describe_image', 'read_image', 'analyze_image', 'image_describe', 'vision', 'ocr'],
    hints: ['image', 'screenshot', 'photo', 'picture', 'visual', 'chart', 'scan'],
  },
]

/** An ADMIN'S OWN WIRING, which always wins over the heuristic below.
 *
 *  Auto-detection by tool name is a convenience so a fresh install works without
 *  a setup step; it is not a thing to be at the mercy of. A server whose search
 *  tool is called `q` is invisible to the heuristic, and a tool called
 *  `web_search` that actually searches an intranet is a false positive. Either
 *  way an admin can say so, and what they say is the answer. */
export type CapabilityProviders = Partial<Record<Capability, { server: string; tool: string } | null>>
export const PROVIDERS_KEY = 'capability_providers'

/** A TOOL TALARIA ITSELF SUPPLIES, which is in nobody's MCP registry.
 *
 *  THE GAP THIS CLOSES. `supplierFor` read the org's registered MCP servers and
 *  nothing else, so on an install with an empty registry — the default, and what
 *  every fresh deployment looks like — it answered `null` for `search` while
 *  SearXNG was running and answering queries on the same box. The consequence
 *  was not cosmetic: `research-search` declares `suppliable: ['search']`, the
 *  sweep asked for a supplier, got null, and handed the model no search tool at
 *  all. The harness that exists to measure tool-driven research measured a model
 *  answering from memory, and the matrix reported models as unable to search on
 *  a deployment that demonstrably can.
 *
 *  IT IS A LAST RESORT, NOT A DEFAULT. The registry is consulted first, because
 *  an org that installed Exa or Tavily chose it and an admin pin beats both.
 *  Talaria's own tool is the floor under that choice, not a competitor to it.
 *
 *  AND IT IS CHECKED, NOT ASSUMED. Every entry is supplied by a caller that has
 *  confirmed the thing actually works here — SearXNG reachable, a model assigned
 *  to the vision role. Claiming reach through a tool that 503s would be a worse
 *  lie than the `null` it replaced, because it converts a red cell an admin can
 *  act on into a green one they cannot. */
export interface PlatformSupply {
  capability: Capability
  server: string
  tool: string
}

export interface ReachDeps {
  servers: () => Promise<McpServer[]>
  providers: () => Promise<CapabilityProviders>
  capabilities: (key: string) => Promise<Partial<Record<Capability, CapabilityFact>>>
  /** Talaria's own tools, already checked — see `capability-platform.ts`. The
   *  floor under the registry, so an install that has registered nothing still
   *  reaches what this deployment can actually do. */
  platform: () => Promise<PlatformSupply[]>
}

const REAL: ReachDeps = {
  servers: listMcpServers,
  providers: () => getSetting<CapabilityProviders>(PROVIDERS_KEY, {}),
  capabilities: getCapabilities,
  platform: platformSupply,
}

const withDeps = (over?: Partial<ReachDeps>): ReachDeps => ({ ...REAL, ...over })

/** Whole-word match, so `search_knowledge` does not answer for `search`. */
const nameMatches = (toolName: string, names: readonly string[]): boolean => {
  const n = toolName.toLowerCase()
  return names.some((want) => n === want || n.endsWith(`_${want}`) || n.startsWith(`${want}_`))
}

/** The registered, ENABLED tool that supplies this capability, or null.
 *
 *  Pure over the server list so the whole rule can be tested without a database
 *  and without an MCP server anywhere near it. */
export function supplierFor(
  capability: Capability,
  servers: readonly McpServer[],
  providers: CapabilityProviders = {},
  platform: readonly PlatformSupply[] = [],
): { server: string; tool: string } | null {
  const pinned = providers[capability]
  // An explicit `null` is an admin saying "nothing supplies this here" — a
  // deliberate answer, and not the same as having said nothing. It silences the
  // platform's own tool too: "nothing supplies this here" means nothing.
  if (pinned === null) return null
  if (pinned) {
    const srv = servers.find((s) => s.name === pinned.server && s.enabled)
    if (srv?.tools.some((t) => t.name === pinned.tool)) return pinned
    // A pin whose server has gone away falls through to the platform's own tool
    // rather than to nothing: the admin's answer to "which supplier" is stale,
    // but their answer to "should this be supplied" was yes.
    return platformFor(capability, platform)
  }

  const rule = TOOL_REACHABLE.find((r) => r.capability === capability)
  if (!rule) return null
  for (const srv of servers) {
    if (!srv.enabled) continue
    for (const tool of srv.tools) {
      if (!nameMatches(tool.name, rule.names)) continue
      // A name match alone is enough when the tool publishes no description;
      // when it does, one corroborating word keeps a same-named intranet search
      // from being read as the live web.
      const desc = (tool.description ?? '').toLowerCase()
      if (desc && !rule.hints.some((h) => desc.includes(h))) continue
      return { server: srv.name, tool: tool.name }
    }
  }
  // Nothing registered offers it. Talaria's own surface is the floor.
  return platformFor(capability, platform)
}

const platformFor = (capability: Capability, platform: readonly PlatformSupply[]): { server: string; tool: string } | null => {
  const own = platform.find((p) => p.capability === capability)
  return own ? { server: own.server, tool: own.tool } : null
}

/** CAN THIS RUN REACH THESE CAPABILITIES, and how.
 *
 *  `keys` are the capability keys the model resolves to — the same endpoint:model
 *  keys `run.ts` derives, passed in rather than re-derived so this file never
 *  becomes a second spelling of that rule. A capability counts as native only
 *  when EVERY key says so, which is the same unanimity `missingCapabilities`
 *  applies: a bare id can land on any endpoint in the pool, and a claim has to
 *  hold for the worst of them. */
export async function reachFor(keys: readonly string[], wanted: readonly Capability[], deps?: Partial<ReachDeps>): Promise<Record<string, Reach>> {
  const d = withDeps(deps)
  const out: Record<string, Reach> = {}
  if (wanted.length === 0) return out

  const facts = await Promise.all(keys.map((k) => d.capabilities(k).catch((): Partial<Record<Capability, CapabilityFact>> => ({}))))
  const nativeYes = (cap: Capability): boolean => keys.length > 0 && facts.every((f) => f[cap]?.value === true)
  const nativeNo = (cap: Capability): boolean => keys.length > 0 && facts.every((f) => f[cap]?.value === false)

  // Only read the registry if something might need a tool. An install with no
  // tool-reachable requirement should not pay for the query.
  const needsTools = wanted.some((c) => TOOL_REACHABLE.some((r) => r.capability === c))
  const [servers, providers, platform] = needsTools
    ? await Promise.all([
        d.servers().catch((): McpServer[] => []),
        d.providers().catch((): CapabilityProviders => ({})),
        d.platform().catch((): PlatformSupply[] => []),
      ])
    : [[] as McpServer[], {} as CapabilityProviders, [] as PlatformSupply[]]

  for (const cap of wanted) {
    const supplier = supplierFor(cap, servers, providers, platform)
    // A NATIVE CLAIM THIS DEPLOYMENT CANNOT CASH loses to a tool that works, and
    // `search` is the capability where that gap is real rather than theoretical.
    //
    // Nearly every model that "has web search" only searches WHEN ASKED — an
    // `web_search_options` block, a provider plugin, an `:online` model suffix.
    // So for a model the catalog or a probe calls search-capable but that nobody
    // ASKS to search, "native" means: it answers from memory and the run ends
    // with no sources. That is the bug this ordering fixes — a deployment with
    // SearXNG up, a model measured at 100% tool calling, and a research run that
    // died having searched nothing.
    //
    // UPDATED, because this comment used to say "Talaria sends none of them" and
    // that is no longer true. The Rust native-search arming
    // (api/src/native_search.rs) now arms what can be armed over an
    // OpenAI-shaped body: OpenRouter's web plugin, and Perplexity, which
    // needs nothing. It is still only those two — OpenAI's switch is the MODEL
    // (`-search-api`) and its parameter 400s elsewhere; Anthropic's search is a
    // server tool on a body shape the compat layer does not expose.
    //
    // SO THE DEFAULT STAYS TOOL-FIRST, and the reason is coverage rather than
    // quality: the tool path works for every model, arming works for two
    // providers. A sonar model on an install with SearXNG registered is looped
    // through the tool rather than spending its own index — which is
    // suboptimal, not broken, and the escape hatch below is exactly for it.
    //
    // THE REFINEMENT, LEFT UNDONE DELIBERATELY: prefer native when it is
    // genuinely ARMED (`canArmNative`), tool otherwise. That is more correct
    // than either ordering this file has had. It is not done here because a
    // capability KEY carries an endpoint NAME, not a provider, so `reachFor`
    // would need a new dependency to ask — and answering the same question in
    // `planSearch` instead would be the second spelling this file's own header
    // warns about.
    //
    // WHY IT IS SAFE FOR THE MODELS THAT REALLY DO BROWSE: a supplier has to be
    // REGISTERED AND CHECKED to exist at all (see `platformSupply`), and an org
    // that would rather spend a sonar model's own index than loop it through a
    // web-search tool says so the way it says everything else here — pin
    // `capability_providers.search` to `null`, which is an admin stating that
    // nothing supplies search in this install, and the native path is all that
    // is left.
    //
    // ONLY `search`. `vision`'s tool stand-in (`describe_image`) is genuinely
    // lossier than a model that reads the image itself, so a model that can see
    // should keep seeing.
    const toolFirst = cap === 'search' && supplier !== null && !nativeNo('tools')
    if (nativeYes(cap) && !toolFirst) {
      out[cap] = { capability: cap, reached: true, via: 'native', supplier: null, detail: `the model does '${cap}' itself` }
      continue
    }

    if (supplier) {
      // THE MODEL STILL HAS TO BE ABLE TO CALL THE TOOL. A search server in
      // front of a model that cannot hold a tool call is not reach — it is a
      // model that will answer from memory with a tool sitting unused beside it,
      // which is the exact failure the search floor exists to prevent.
      if (nativeNo('tools')) {
        out[cap] = {
          capability: cap,
          reached: false,
          via: null,
          supplier: null,
          detail: `'${supplier.server}.${supplier.tool}' could supply '${cap}', but this model is recorded as unable to call tools.`,
        }
        continue
      }
      out[cap] = {
        capability: cap,
        reached: true,
        via: 'tool',
        supplier,
        detail: `the model calls '${supplier.server}.${supplier.tool}' for it`,
      }
      continue
    }

    // NOT REACHED, and the sentence has to say which of the two reasons — the
    // model cannot, or the org has not installed the thing that could.
    const reachable = TOOL_REACHABLE.some((r) => r.capability === cap)
    out[cap] = {
      capability: cap,
      reached: false,
      via: null,
      supplier: null,
      detail: reachable
        ? `nothing here reaches '${cap}': the model does not do it natively and no enabled MCP server offers a tool for it. Register one, or assign a model that does '${cap}' itself.`
        : nativeNo(cap)
          ? `the model is recorded as not supporting '${cap}', and nothing can supply it on the model's behalf.`
          : `nothing has measured '${cap}' on this model.`,
    }
  }
  return out
}
