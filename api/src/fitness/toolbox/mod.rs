// THE FITNESS TOOLBOX — the isolated Talaria a dry run happens inside. Port of
// ui/src/server/fitness/toolbox/*.
//
// WHY IT IS ITS OWN MODULE TREE. These backends exist ONLY for the fitness
// suite: production runs the real MCP surface against the real database, and
// the one thing a benchmark must never do is move a real ticket. Nothing under
// api/src outside fitness/ may import it, and nothing in it may import a pool,
// a redis handle or an HTTP client — isolation is total and is the point.

pub mod world;

// The sandbox backends themselves (sandbox.ts's tool dispatch over the world),
// the coding workspace (hermes-tools.ts), the credential surface
// (credential-tools.ts) and the dry-run transport (dry-run.ts) cross with the
// sweep engine that drives them.
