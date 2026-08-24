import { describe, expect, it } from 'vitest'
import { agentFromAddress, emailDomainOf, plusAddress, plusTag } from './aliasing'

// The address scheme, pinned. These strings ARE the product surface —
// org+triage@domain is what lands in a customer's From header — so the rules
// are tested as contracts, not implementation details.

describe('emailDomainOf', () => {
  it('lowercases and takes the last @', () => {
    expect(emailDomainOf('Jon@OutcropLabs.COM')).toBe('outcroplabs.com')
    expect(emailDomainOf('a+b@x.co')).toBe('x.co')
  })
  it('is null for anything without a usable local@domain shape', () => {
    expect(emailDomainOf(null)).toBeNull()
    expect(emailDomainOf(undefined)).toBeNull()
    expect(emailDomainOf('')).toBeNull()
    expect(emailDomainOf('nope')).toBeNull()
    expect(emailDomainOf('@x.co')).toBeNull()
    expect(emailDomainOf('a@')).toBeNull()
  })
})

describe('plusTag', () => {
  it('folds a slug to lowercase [a-z0-9-] with single dashes', () => {
    expect(plusTag('Field Ops')).toBe('field-ops')
    expect(plusTag('SUPPORT')).toBe('support')
    expect(plusTag('weird--!!name')).toBe('weird-name')
  })
  it('trims leading and trailing dashes', () => {
    expect(plusTag('--edge--')).toBe('edge')
  })
})

describe('plusAddress', () => {
  it('derives the org account’s plus-address for a tag', () => {
    expect(plusAddress('jon@outcroplabs.com', 'triage')).toBe('jon+triage@outcroplabs.com')
  })
  it('replaces, never stacks, an existing plus-tag on the org email', () => {
    expect(plusAddress('jon+old@outcroplabs.com', 'triage')).toBe('jon+triage@outcroplabs.com')
  })
  it('folds the tag like plusTag does', () => {
    expect(plusAddress('jon@x.co', 'Field Ops!')).toBe('jon+field-ops@x.co')
  })
  it('is null without a usable org email or tag', () => {
    expect(plusAddress('broken', 't')).toBeNull()
    expect(plusAddress('a@b.c', '')).toBeNull()
  })
})

describe('agentFromAddress', () => {
  it('prefers the stored override', () => {
    expect(agentFromAddress({ slug: 'triage', emailAlias: 'support@outcroplabs.com' }, 'jon@outcroplabs.com')).toBe(
      'support@outcroplabs.com',
    )
  })
  it('derives the plus-address when no override is set', () => {
    expect(agentFromAddress({ slug: 'triage', emailAlias: null }, 'jon@outcroplabs.com')).toBe(
      'jon+triage@outcroplabs.com',
    )
    expect(agentFromAddress({ slug: 'triage' }, 'jon@outcroplabs.com')).toBe('jon+triage@outcroplabs.com')
  })
  it('blank override means unset, not an empty From', () => {
    expect(agentFromAddress({ slug: 'triage', emailAlias: '   ' }, 'jon@outcroplabs.com')).toBe(
      'jon+triage@outcroplabs.com',
    )
  })
  it('is null with no org email — the caller falls back to sendAs', () => {
    expect(agentFromAddress({ slug: 'triage', emailAlias: null }, null)).toBeNull()
  })
})
