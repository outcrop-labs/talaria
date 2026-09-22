- **The api package image failed to compile on `main`.** `hermes_skills.rs`
  `include_str!`s `scripts/hermes-skill-authority.json` from repo root;
  `package.Dockerfile` had flattened `api/` onto `/repo`, so the path was
  `/scripts/...` and missing. The build now keeps the repo layout
  (`/repo/api` + `/repo/scripts/...`). Verified: the previous `main` package
  job failed on that exact error; this file is the fix.
