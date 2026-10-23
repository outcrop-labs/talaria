// The route matcher decodes a path once, per segment (api-route.ts). It used
// to be handed a path the fetch handler had already decoded, and decoded each
// part again: `%25` reached the second decode as a bare `%` and threw — a 500
// before any route ran — and `%2F` was a separator by the time it split.
import { describe, expect, it } from 'vitest'
import { compileRoute, defineApi, matchRoute, matchSegments, pathSegments } from './api-route'

const ok = () => new Response(null)
const splat = compileRoute(defineApi('/api/apps/$app/$', { GET: ok }))
const param = compileRoute(defineApi('/api/mcp/gw/$server', { POST: ok }))

describe('pathSegments', () => {
  it('splits the raw path, then decodes each segment exactly once', () => {
    expect(pathSegments('/api/apps/crm/files/100%25')).toEqual(['api', 'apps', 'crm', 'files', '100%'])
    expect(pathSegments('/api/apps/crm/a%2525')).toEqual(['api', 'apps', 'crm', 'a%25'])
    expect(pathSegments('/api/apps/crm/a%2Fb')).toEqual(['api', 'apps', 'crm', 'a/b'])
    expect(pathSegments('//api///x/')).toEqual(['api', 'x'])
  })

  it('is null for a malformed escape', () => {
    expect(pathSegments('/api/apps/crm/100%')).toBeNull()
    expect(pathSegments('/api/apps/crm/%E0%A4%A')).toBeNull()
  })
})

describe('matchRoute', () => {
  it('a literal % reaches the route instead of throwing', () => {
    expect(matchRoute(splat, '/api/apps/crm/files/100%25')).toEqual({ app: 'crm', _splat: 'files/100%' })
    expect(matchRoute(param, '/api/mcp/gw/app-100%25')).toEqual({ server: 'app-100%' })
  })

  it('an encoded slash stays inside its segment', () => {
    expect(matchRoute(param, '/api/mcp/gw/a%2Fb')).toEqual({ server: 'a/b' })
    expect(matchRoute(param, '/api/mcp/gw/a/b')).toBeNull()
  })

  it('statics compare against the decoded segment, and a malformed path matches nothing', () => {
    expect(matchRoute(splat, '/api/%61pps/crm/x')).toEqual({ app: 'crm', _splat: 'x' })
    expect(matchRoute(splat, '/api/apps/crm/100%')).toBeNull()
    expect(matchSegments(splat, ['api', 'apps'])).toBeNull()
    expect(matchSegments(splat, ['api', 'apps', 'crm'])).toEqual({ app: 'crm', _splat: '' })
  })
})
