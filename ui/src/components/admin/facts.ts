/** One line of a `FactsList`. `mono` marks an identifier: a URL, a path, an id. */
export interface Fact {
  label: string
  value: string
  mono?: boolean
}
