// THE TOOLKIT'S OWN NAME FOR A TOOL, however the agent's MCP client spelled it.
//
// Hermes registers every MCP tool as `mcp__<server>__<tool>` and streams that
// registry name, so a Talaria writer reaches the chat stream as
// `mcp__talaria__create_document` — and a set keyed on `create_document` never
// matched it: the Work pane did not re-read its files when the agent made one
// mid-turn. The api reads the tool-events door's names the same way
// (talaria-chips `bare_tool_name`). Only the toolkit's own server is stripped,
// plus the single-underscore display spelling some Hermes surfaces use;
// another server's tool keeps its prefix, because it is not a toolkit tool.

const TOOLKIT_PREFIX = /^(?:mcp__talaria__|mcp_talaria_)(?=.)/

export const bareToolName = (name: string): string => name.trim().replace(TOOLKIT_PREFIX, '')
