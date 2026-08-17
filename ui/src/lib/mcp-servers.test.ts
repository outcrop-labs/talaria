// Which side of the MCP page a server lands on.
//
// The split is not cosmetic: the External tab is the only one that offers "add"
// and "repoint", because those are the only servers where that means anything.
// A misclassified internal server would offer to repoint something whose URL is
// a routing token; a misclassified external one would hide the controls that
// are its whole point.
import { describe, expect, it } from 'vitest'
import { isInternalServer, type ServerOrigin } from './mcp-servers'

const server = (over: Partial<ServerOrigin>): ServerOrigin => ({
  builtin: false,
  appSlug: null,
  url: 'https://mcp.example.com/mcp',
  ...over,
})

describe('isInternalServer', () => {
  it("counts Talaria's own toolkit as internal", () => {
    // Its URL is loopback http, so the flag is what has to carry this.
    expect(isInternalServer(server({ builtin: true, url: 'http://127.0.0.1:8770/mcp' }))).toBe(true)
  })

  it('counts an app-published server as internal', () => {
    expect(isInternalServer(server({ appSlug: 'contacts', url: 'https://anything/mcp' }))).toBe(true)
  })

  it('counts the Workbench as internal', () => {
    // The case neither flag covers: in-process, but carrying a routing token
    // rather than a hostname. The scheme test is what catches it.
    expect(isInternalServer(server({ url: 'talaria-workbench://core' }))).toBe(true)
  })

  it('counts a registered third-party endpoint as external', () => {
    expect(isInternalServer(server({ url: 'https://mcp.linear.app/mcp' }))).toBe(false)
    expect(isInternalServer(server({ url: 'http://mcp.corp.internal/mcp' }))).toBe(false)
  })

  it('does not treat a hostname that merely mentions talaria as internal', () => {
    // Substring matching on the URL would have; the scheme test does not, and a
    // third-party host is free to call itself whatever it likes.
    expect(isInternalServer(server({ url: 'https://talaria-workbench.evil.example/mcp' }))).toBe(false)
  })

  it('is case-insensitive about the scheme', () => {
    expect(isInternalServer(server({ url: 'HTTPS://mcp.example.com/mcp' }))).toBe(false)
  })

  it('partitions a mixed registry with nothing left over', () => {
    const rows = [
      server({ builtin: true, url: 'http://127.0.0.1:8770/mcp' }),
      server({ url: 'talaria-workbench://core' }),
      server({ appSlug: 'contacts' }),
      server({ url: 'https://mcp.linear.app/mcp' }),
      server({ url: 'https://mcp.notion.com/mcp' }),
    ]
    const internal = rows.filter(isInternalServer)
    const external = rows.filter((s) => !isInternalServer(s))
    expect(internal).toHaveLength(3)
    expect(external).toHaveLength(2)
    expect(internal.length + external.length).toBe(rows.length)
  })
})
