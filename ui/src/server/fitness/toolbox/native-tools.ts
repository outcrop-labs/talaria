// THE PLATFORM'S OWN TOOL SURFACE — what Talaria hands a model ITSELF.
//
// THE SPLIT THIS FILE EXISTS TO MAKE. Talaria puts tools in front of a model on
// two completely different paths, and until this file they were discussed as if
// they were one thing:
//
//   HERMES AGENTS — a containerized persona connects to `mcp/src/index.ts` over
//     MCP and holds all forty-four workspace verbs: tickets, channels, the
//     knowledgebase, documents, Google, board governance. Talaria never sees the
//     loop; the persona runs it and reports tool NAMES. Modelled in
//     `talaria-tools.ts`, plus the coding harness's own file tools in
//     `hermes-tools.ts`.
//
//   NATIVE PLATFORM AGENTS — a harness in this repository (`harness/defs/*`)
//     that puts tool DEFINITIONS on the request itself (`TransportRequest.
//     toolDefs`) and runs the loop in-process, watching every call and every
//     result. This file is that surface, entire.
//
// WHY THE DISTINCTION IS NOT COSMETIC. It decides what a fitness verdict means.
// `tools` and `tool-select` measured through the MCP path say "this model can
// drive a fleet agent"; measured through this path they say "this model can be
// the platform's own research/briefing brain". A deployment can be good at one
// and useless at the other — a gateway that strips `tools` from the request kills
// every native harness while leaving every Hermes persona untouched, because the
// persona's loop never travels through that gateway at all.
//
// AND IT IS SHORT ON PURPOSE — IT IS ALSO THE FINDING. One tool. Talaria's own
// harnesses are, with a single exception, structured single-shot calls: render a
// prompt, parse a contract, return. The one native loop is research's web
// search. Every other capability an in-UI agent appears to have (reading a
// ticket, posting to a channel) is either a Hermes persona doing it or Talaria
// doing it in code around the model, not the model calling a tool. Anyone
// proposing to give native agents a real toolkit should start by adding to this
// file — and the shortness of this list is the honest measure of how much is
// missing today.
import type { ToolDefinition } from '../../harness/transport'
import { SEARCH_TOOL_DESCRIPTION, SEARCH_TOOL_SCHEMA } from '../../harness/defs/research'

/** A native tool, plus who supplies it. */
export interface NativeTool extends ToolDefinition {
  name: string
  /** WHERE THE NAME COMES FROM AT RUNTIME.
   *
   *  'registry' means the org's MCP registry supplies the real name — the search
   *  supplier is whatever web-search server the org installed, so the harness
   *  sends `supplier.tool` and this file's `name` is only the shape. A fixture
   *  therefore must not assert on the literal name; it asserts that the ONE
   *  offered tool was called. */
  supplied: 'registry' | 'platform'
  /** The harness that offers it, so a reader can find the loop. */
  harness: string
}

// THE DEFINITION IS THE HARNESS'S, NOT A COPY OF IT. `research.ts` owns the
// search tool's description and schema because it is the file that sends them;
// this file imports them so the roster cannot drift from the surface. The whole
// reason `talaria-tools.ts` needs a sync test is that the MCP toolkit lives in
// another package and cannot be imported — nothing forces that here, so nothing
// is copied here.
export const NATIVE_TOOLS: readonly NativeTool[] = [
  {
    name: 'web_search',
    supplied: 'registry',
    harness: 'research:search',
    description: SEARCH_TOOL_DESCRIPTION,
    parameters: SEARCH_TOOL_SCHEMA as unknown as Record<string, unknown>,
  },
] as const

export const nativeToolNames = (): string[] => NATIVE_TOOLS.map((t) => t.name)
