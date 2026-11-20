import { describe, expect, it } from 'vitest'
import { bareToolName } from './toolkit-tools'

describe('bareToolName', () => {
  it('reads a toolkit tool by its own name whatever the MCP client called it', () => {
    for (const spelling of ['create_document', 'mcp__talaria__create_document', 'mcp_talaria_create_document', ' mcp__talaria__create_document ']) {
      expect(bareToolName(spelling)).toBe('create_document')
    }
  })
  it('leaves another server\'s tool, and an empty remainder, alone', () => {
    expect(bareToolName('mcp__workspace__create_document')).toBe('mcp__workspace__create_document')
    expect(bareToolName('mcp__talaria__')).toBe('mcp__talaria__')
    expect(bareToolName('web_search')).toBe('web_search')
  })
})
