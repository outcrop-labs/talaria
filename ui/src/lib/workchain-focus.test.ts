import { describe, expect, it } from 'vitest'
import { pickFocusedChain, type Workchain } from '@/lib/workchain-rules'

// Pure-helper tests for the focused-chain pick: which chain a focus-holding
// view should render. The pick keeps the focused id while it still names a
// chain in the list; null focus, an unknown id, and the stale id a delete
// leaves behind all fall to the FIRST chain in list order, so a view with
// chains never sits focused on nothing. The focus state itself stays the
// component's; these tests pin the pure derivation it consumes.

const chainId = (id: string): Pick<Workchain, 'id'> => ({ id })

describe('pickFocusedChain', () => {
  it('an empty list has nothing to focus, focused id or not', () => {
    expect(pickFocusedChain([], null)).toBeNull()
    expect(pickFocusedChain([], 'wc_1')).toBeNull()
  })

  it('null focus falls to the first chain in list order', () => {
    expect(pickFocusedChain([chainId('wc_1'), chainId('wc_2')], null)).toBe('wc_1')
  })

  it('a focus naming no chain in the list falls to the first', () => {
    expect(pickFocusedChain([chainId('wc_1'), chainId('wc_2')], 'wc_elsewhere')).toBe('wc_1')
  })

  it('a valid focus is kept even when it is not the first chain', () => {
    expect(pickFocusedChain([chainId('wc_1'), chainId('wc_2'), chainId('wc_3')], 'wc_3')).toBe('wc_3')
  })

  it('a stale focus — its chain was deleted — falls to the first survivor', () => {
    // wc_2 held the focus, then got deleted; the id outlived its chain.
    expect(pickFocusedChain([chainId('wc_1'), chainId('wc_3')], 'wc_2')).toBe('wc_1')
  })
})