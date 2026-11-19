import { describe, expect, it } from 'vitest'
import { actorLabel } from './actor'

describe('actorLabel', () => {
  it("reads an agent's slug as its name", () => {
    expect(actorLabel('relay')).toBe('Relay')
    expect(actorLabel('harbor-support')).toBe('Harbor')
  })

  it("leaves a person's address or name as served", () => {
    expect(actorLabel('owen@larkspurfreight.co')).toBe('owen@larkspurfreight.co')
    expect(actorLabel('Maren Okafor')).toBe('Maren Okafor')
  })
})
