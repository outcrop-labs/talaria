// THE FITNESS TOOLBOX — the isolated Talaria a dry run happens inside.
//
// WHY IT IS ITS OWN MODULE TREE. These backends exist ONLY for the fitness
// suite: production runs the real MCP surface against the real database, and
// the one thing a benchmark must never do is move a real ticket. Nothing under
// api/src outside fitness/ may import it, and nothing in it may import a pool,
// a redis handle or an HTTP client — isolation is total and is the point.

pub mod credential_tools;
pub mod dry_run;
pub mod hermes_tools;
pub mod sandbox;
pub mod talaria_tools;
pub mod world;

// Reading order: `world` (the in-memory Talaria), `talaria_tools` (the
// catalog, locked to mcp/src/index.ts by a sync test), `sandbox` (the dispatch
// over both), then the two other surfaces — `hermes_tools` (files and a test
// runner), `credential_tools` (a credential spent at an external service) —
// and `dry_run`, the loop that drives any of them through a transport.
