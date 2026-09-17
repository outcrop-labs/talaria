import { describe, expect, it } from 'vitest'
import {
  composeHeader,
  composeHeaders,
  headerFields,
  literalHeaders,
  templateVars,
  type HeaderDecl,
} from './mcp-headers'

// The registry's DOMINANT declaration (Smithery's, verbatim shape): a fixed
// Bearer prefix around one secret variable.
const smithery = (over: Partial<HeaderDecl> = {}): HeaderDecl => ({
  name: 'Authorization',
  description: 'Bearer token for Smithery authentication',
  isRequired: true,
  isSecret: true,
  placeholder: null,
  default: null,
  choices: null,
  value: 'Bearer {smithery_api_key}',
  variables: null,
  ...over,
})

describe('templateVars', () => {
  it('collects variables in order, deduped', () => {
    expect(templateVars('Bearer {api_key}')).toEqual(['api_key'])
    expect(templateVars('{a}+{b}/{a}')).toEqual(['a', 'b'])
    expect(templateVars('{not-a_var}')).toEqual(['not-a_var'])
    // Double braces / spaces are not variables — they stay literal.
    expect(templateVars('key={{ not-a-var }}')).toEqual([])
    expect(templateVars('literal')).toEqual([])
    expect(templateVars('oops {unterminated')).toEqual([])
    expect(templateVars('empty {} braces')).toEqual([])
  })
})

describe('headerFields', () => {
  it('one field per template variable, secret-ness and metadata inherited or from the variables map', () => {
    const h = smithery({
      variables: { smithery_api_key: { description: null, isSecret: true, placeholder: 'sk-…', default: null, choices: null } },
    })
    const [f] = headerFields(h)
    expect(f!.key).toBe('Authorization::smithery_api_key')
    expect(f!.label).toBe('smithery_api_key')
    expect(f!.header).toBe('Authorization')
    expect(f!.isSecret).toBe(true) // inherited from the header
    expect(f!.placeholder).toBe('sk-…') // from the variables map
    // Without a variables entry the header's own description still guides.
    const bare = headerFields(smithery())[0]!
    expect(bare.description).toBe('Bearer token for Smithery authentication')
  })

  it('a declaration without a value is one whole-header field — yesterday’s shape', () => {
    const fields = headerFields(smithery({ value: null, placeholder: 'paste the token' }))
    expect(fields).toHaveLength(1)
    expect(fields[0]!.key).toBe('Authorization')
    expect(fields[0]!.label).toBe('Authorization')
    expect(fields[0]!.placeholder).toBe('paste the token')
  })

  it('a fixed value renders no field at all', () => {
    expect(headerFields(smithery({ value: '2', isRequired: true }))).toHaveLength(0)
  })
})

describe('composeHeader', () => {
  it('wraps the typed key in the template', () => {
    const h = smithery()
    expect(composeHeader(h, { 'Authorization::smithery_api_key': 'sk-123' })).toBe('Bearer sk-123')
  })

  it('keeps unresolved braces — never a half-composed secret', () => {
    expect(composeHeader(smithery(), {})).toBe('Bearer {smithery_api_key}')
    expect(composeHeader(smithery(), { 'Authorization::smithery_api_key': '  ' })).toBe(
      'Bearer {smithery_api_key}',
    )
  })

  it('a value-less header passes the typed value through', () => {
    expect(composeHeader(smithery({ value: null }), { Authorization: 'Bearer abc' })).toBe('Bearer abc')
  })
})

describe('composeHeaders + literalHeaders', () => {
  it('auto-applies fixed headers and skips all-empty declarations', () => {
    const decls = [
      smithery(),
      { ...smithery(), name: 'X-Api-Version', value: '2', description: null },
      { ...smithery(), name: 'X-Optional', value: 'opt {opt_key}', isRequired: false },
    ]
    // Only the fixed header: nothing typed, nothing half-stored.
    expect(composeHeaders(decls, {})).toEqual({ 'X-Api-Version': '2' })
    expect(literalHeaders(decls)).toEqual({ 'X-Api-Version': '2' })
    expect(composeHeaders(decls, { 'Authorization::smithery_api_key': 'sk-9' })).toEqual({
      'X-Api-Version': '2',
      Authorization: 'Bearer sk-9',
    })
  })
})
