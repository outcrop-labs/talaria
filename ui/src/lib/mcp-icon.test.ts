import { describe, expect, it } from 'vitest'
import { iconDomains, registrableDomain } from './mcp-icon'

describe('iconDomains', () => {
  it('asks for the publisher before anything the address says', () => {
    expect(iconDomains({ publisher: 'notion.com', url: 'https://mcp.notion.com/mcp' })).toEqual(['notion.com'])
    expect(iconDomains({ publisher: 'Sentry.io', url: 'https://mcp.sentry.dev/mcp' })).toEqual(['sentry.io', 'sentry.dev'])
  })

  it('falls back to the address host, then the domain it is registered under', () => {
    expect(iconDomains({ url: 'https://qbo-mcp.larkspurfreight.co/mcp' })).toEqual(['qbo-mcp.larkspurfreight.co', 'larkspurfreight.co'])
    expect(iconDomains({ url: 'https://tools.acme.co.uk/mcp' })).toEqual(['tools.acme.co.uk', 'acme.co.uk'])
  })

  it('reads a leading mcp label as the protocol, not the product', () => {
    expect(iconDomains({ url: 'https://mcp.slack.com/mcp' })).toEqual(['slack.com'])
    expect(iconDomains({ url: 'https://mcp.linear.app/mcp' })).toEqual(['linear.app'])
  })

  it('knows the brand behind an api host that serves no icon', () => {
    expect(iconDomains({ url: 'https://api.githubcopilot.com/mcp/' })).toEqual(['github.com', 'api.githubcopilot.com', 'githubcopilot.com'])
    expect(iconDomains({ url: 'https://drivemcp.googleapis.com/mcp' })).toEqual(['google.com', 'drivemcp.googleapis.com', 'googleapis.com'])
  })

  it('lets a publisher the row carries win over the brand it would have guessed', () => {
    expect(iconDomains({ publisher: 'example.com', url: 'https://api.githubcopilot.com/mcp/' })[0]).toBe('example.com')
  })

  it('has nothing to ask for an address with no public host', () => {
    expect(iconDomains({ url: 'talaria-pkg://warehouse' })).toEqual([])
    expect(iconDomains({ url: 'talaria://boards' })).toEqual([])
    expect(iconDomains({ url: 'http://10.0.0.4:8080/mcp' })).toEqual([])
    expect(iconDomains({ url: 'http://toolkit.internal/mcp' })).toEqual([])
    expect(iconDomains({ url: 'not a url' })).toEqual([])
    expect(iconDomains({})).toEqual([])
  })

  it('still asks for a publisher when the address has no host', () => {
    expect(iconDomains({ publisher: 'postgresql.org', url: 'talaria-pkg://warehouse' })).toEqual(['postgresql.org'])
  })
})

describe('registrableDomain', () => {
  it('keeps two labels, or three under a two-label suffix', () => {
    expect(registrableDomain('drivemcp.googleapis.com')).toBe('googleapis.com')
    expect(registrableDomain('linear.app')).toBe('linear.app')
    expect(registrableDomain('a.b.example.co.uk')).toBe('example.co.uk')
    expect(registrableDomain('API.GitHubCopilot.com.')).toBe('githubcopilot.com')
  })
})
