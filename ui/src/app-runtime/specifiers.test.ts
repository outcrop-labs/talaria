import { describe, expect, it } from 'vitest'
import { bundledShared, clientSpecifiers, shimFileName, svelteClientSpecifiers } from './specifiers'

describe('app-runtime specifiers', () => {
  it('includes svelte internals the compiler injects', () => {
    const svelte = svelteClientSpecifiers()
    expect(svelte).toContain('svelte')
    expect(svelte).toContain('svelte/internal/client')
    expect(svelte).toContain('svelte/internal/disclose-version')
    expect(svelte).not.toContain('svelte/compiler')
    expect(svelte).not.toContain('svelte/server')
  })

  it('names shims stably', () => {
    expect(shimFileName('@talaria/sdk')).toBe('rt-talaria-sdk')
    expect(shimFileName('svelte/internal/client')).toBe('rt-svelte-internal-client')
  })

  it('flags a bundled svelte module id', () => {
    expect(bundledShared(['/repo/ui/node_modules/svelte/src/internal/client/index.js'])).toHaveLength(1)
    expect(bundledShared(['/repo/apps/contacts/app.ts'])).toEqual([])
  })

  it('lists the host SDK as a client specifier', () => {
    expect(clientSpecifiers()).toContain('@talaria/sdk')
  })
})
