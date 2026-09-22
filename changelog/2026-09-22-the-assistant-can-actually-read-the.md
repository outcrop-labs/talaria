- **The assistant can actually read the mail it manages.** Two new fleet
  tools: `read_email` (one full message by id — headers plus the complete
  plain-text body, decoded from Gmail's nested MIME tree, capped at 20k
  characters) because the listing tool only ever returned snippets, and
  `search_drive` (find files by name in the Drive the agent acts for, with
  links — read-only). Both are modelled and simulated in the fitness toolbox,
  same as every tool in the kit, and both refuse a legacy shared-key caller
  the way the existing Google tools do.
