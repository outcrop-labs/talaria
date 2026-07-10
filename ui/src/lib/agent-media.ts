// Agent-produced media in chat. Hermes agents reference files they create as
// "MEDIA:<absolute path>" in their replies; rewrite image references into
// inline markdown images served out of the agent's container
// (/api/agent-media/:model?path=). Non-image tokens stay as-is; ordinary
// remote image URLs already render through Markdown untouched.
const IMAGE_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp'])

export function resolveAgentMedia(content: string, agentModel: string): string {
  return content.replace(/MEDIA:(\/[^\s"')\]>]+)/g, (token, path: string) => {
    const ext = path.split('.').pop()?.toLowerCase() ?? ''
    if (!IMAGE_EXT.has(ext)) return token
    return `![agent media](/api/agent-media/${encodeURIComponent(agentModel)}?path=${encodeURIComponent(path)})`
  })
}
