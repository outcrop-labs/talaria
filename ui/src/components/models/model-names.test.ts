import { describe, expect, it } from 'vitest'
import { bareModelId, modelNameIndex } from './model-names.svelte'

const model = (id: string, endpoints: string[], qualified: boolean, label?: string) => ({ id, endpoints, qualified, label })

describe('modelNameIndex', () => {
  const names = modelNameIndex([
    model('Anthropic/claude-sonnet-5-5', ['Anthropic'], true, 'Claude Sonnet 5.5'),
    model('OpenRouter/perplexity/sonar', ['OpenRouter'], true, 'Sonar'),
    model('gpu-01 (vLLM)/qwen3-coder-30b', ['gpu-01 (vLLM)'], true),
    model('gpt-5.2', ['OpenAI', 'Azure'], false, 'GPT-5.2'),
    model('echo', ['Local'], false, 'echo'),
  ])

  it('names a model by its qualified id and by its bare id', () => {
    expect(names.get('Anthropic/claude-sonnet-5-5')).toBe('Claude Sonnet 5.5')
    expect(names.get('claude-sonnet-5-5')).toBe('Claude Sonnet 5.5')
    expect(names.get('gpt-5.2')).toBe('GPT-5.2')
  })

  it('keeps the slash a bare id carries', () => {
    expect(names.get('perplexity/sonar')).toBe('Sonar')
    expect(names.has('sonar')).toBe(false)
  })

  it('has no name for a model the catalog does not name, or names with its own id', () => {
    expect(names.has('qwen3-coder-30b')).toBe(false)
    expect(names.has('echo')).toBe(false)
  })
})

describe('bareModelId', () => {
  it('strips the endpoint, including one whose name holds a slash', () => {
    expect(bareModelId(model('lab/gpu/llama-4', ['lab/gpu'], true))).toBe('llama-4')
    expect(bareModelId(model('OpenAI/gpt-5.2', ['OpenAI'], true))).toBe('gpt-5.2')
  })

  it('leaves an unqualified id alone', () => {
    expect(bareModelId(model('perplexity/sonar', ['OpenRouter', 'Perplexity'], false))).toBe('perplexity/sonar')
  })
})
