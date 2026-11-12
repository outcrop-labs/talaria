-- The git-hosts rollback mirror, checked on a migrated database.
--
--   docker exec -i talaria-mig-pg psql -U talaria -d talaria \
--     [-v seeded='<the github_config the baseline database held>'] \
--     < ui/scripts/check-git-hosts-mirror.sql
--
-- WHY. MIGRATIONS 526-536 (ui/src/server/db/pg.ts, "The git-hosts move, made
-- roll-safe", #651) put back what the previous release reads while both
-- releases serve one database: DEFAULT 'github' on four host_id columns, the
-- app_settings.github_config the 2026-10-27 block deleted, and two triggers
-- that keep that setting and the `github` git_hosts row one value. The
-- migrations job proves the statements apply, and the snapshot pins their
-- text. Neither shows that they do anything. This file does, on the database
-- the job just migrated:
--   1. every host_id the previous release inserts without defaults to 'github',
--      and an insert that names no host lands there;
--   2. at rest, the setting is the `github` row with its orgs folded back in,
--      and with `seeded` (the upgrade pass), it is exactly what the baseline
--      held before the move;
--   3. a write to the row (this release: connect, rotate, clear) rewrites the
--      setting;
--   4. a write to the setting, in the previous release's own statement shape
--      (talaria-settings' set_setting), rewrites the row, on its INSERT path
--      and its UPDATE path, and the setting keeps what was written;
--   5. removing the row empties the setting, and a setting write afterwards
--      makes the row again.
--
-- Everything it writes is inside one transaction that it rolls back, so the
-- job's later steps see the database the migrations left. A failed check
-- raises an exception that names what it saw, and ON_ERROR_STOP turns that
-- into a non-zero exit. Values are placeholders, never real tokens.
--
-- DELETE THIS FILE, and its step in ci.yml's `migrations (replay + upgrade)`
-- job, in the release that ships the contract: dropping the two mirrors and
-- the defaults, and deleting the setting.

\set ON_ERROR_STOP on
\if :{?seeded}
\else
  \set seeded ''
\endif

begin;
select set_config('mirror_check.seeded', :'seeded', true) \g /dev/null

-- 1 · The defaults, read from the catalog, then used.
do $$
declare
  t text;
  d text;
begin
  foreach t in array array['workbench_repos', 'workbench_repo_flow', 'workbench_jobs', 'workbench_repo_requests'] loop
    select column_default into d
      from information_schema.columns
     where table_schema = 'public' and table_name = t and column_name = 'host_id';
    if d is distinct from '''github''::text' then
      raise exception '%.host_id defaults to %, want ''github''', t, coalesce(d, 'nothing');
    end if;
  end loop;
end $$;

insert into workbench_repo_flow (repo) values ('mirror-check/no-host');
do $$
declare
  h text;
begin
  select host_id into h from workbench_repo_flow where repo = 'mirror-check/no-host';
  if h is distinct from 'github' then
    raise exception 'a workbench_repo_flow row inserted with no host_id got %, want github', coalesce(h, 'nothing');
  end if;
end $$;
-- Out again before the row is removed in 5: it would hold the foreign key.
delete from workbench_repo_flow where repo = 'mirror-check/no-host';

-- 2 · At rest, after the pass.
do $$
declare
  auth jsonb;
  want jsonb;
  got jsonb;
  seeded text := current_setting('mirror_check.seeded');
begin
  select h.auth, h.auth || jsonb_build_object('repoCreationOrgs', to_jsonb(h.repo_creation_orgs))
    into auth, want
    from git_hosts h
   where h.id = 'github' and h.kind = 'github';
  if want is null then
    raise exception 'no github git_hosts row after migrating';
  end if;
  select value into got from app_settings where key = 'github_config';
  if seeded <> '' and got is distinct from seeded::jsonb then
    raise exception 'github_config after the upgrade is %, want what the baseline held: %', coalesce(got::text, 'absent'), seeded;
  end if;
  if auth = '{}'::jsonb then
    -- An unconfigured row is copied back as nothing (statement 1 skips it),
    -- which the previous release reads the same way: not connected.
    if got is not null then
      raise exception 'the github row is unconfigured, but github_config is %', got;
    end if;
  elsif got is distinct from want then
    raise exception 'github_config is %, but the github row folds back to %', coalesce(got::text, 'absent'), want;
  end if;
end $$;

-- 3 · A write to the row rewrites the setting.
update git_hosts
   set auth = '{"mode": "pat", "pat": {"tokenEnc": "v1:mirror-check-row"}, "app": {"appId": "", "installationIds": [], "privateKeyEnc": null}}'::jsonb,
       repo_creation_orgs = '{mirror-check-a,mirror-check-b}',
       updated_at = now()
 where id = 'github';
do $$
declare
  got jsonb;
begin
  select value into got from app_settings where key = 'github_config';
  if got is distinct from '{"mode": "pat", "pat": {"tokenEnc": "v1:mirror-check-row"}, "app": {"appId": "", "installationIds": [], "privateKeyEnc": null}, "repoCreationOrgs": ["mirror-check-a", "mirror-check-b"]}'::jsonb then
    raise exception 'after a write to the github row, github_config is %', coalesce(got::text, 'absent');
  end if;
end $$;

-- 4 · A write to the setting rewrites the row: first the INSERT path (no
--     setting yet; deleting one fires nothing), then the UPDATE path.
delete from app_settings where key = 'github_config';
insert into app_settings (key, value)
values ('github_config', '{"mode": "app", "pat": {"tokenEnc": null}, "app": {"appId": "4242", "installationIds": ["99"], "privateKeyEnc": "v1:mirror-check-insert"}, "repoCreationOrgs": ["mirror-check-c"]}')
on conflict (key) do update set value = excluded.value, updated_at = now();
do $$
declare
  a jsonb;
  o text[];
begin
  select auth, repo_creation_orgs into a, o from git_hosts where id = 'github';
  if a is distinct from '{"mode": "app", "pat": {"tokenEnc": null}, "app": {"appId": "4242", "installationIds": ["99"], "privateKeyEnc": "v1:mirror-check-insert"}}'::jsonb
     or o is distinct from '{mirror-check-c}'::text[] then
    raise exception 'after inserting github_config, the github row holds auth % and orgs %', a, o;
  end if;
end $$;

insert into app_settings (key, value)
values ('github_config', '{"mode": "app", "pat": {"tokenEnc": null}, "app": {"appId": "4242", "installationIds": ["99", "100"], "privateKeyEnc": "v1:mirror-check-update"}, "repoCreationOrgs": []}')
on conflict (key) do update set value = excluded.value, updated_at = now();
do $$
declare
  a jsonb;
  o text[];
  got jsonb;
begin
  select auth, repo_creation_orgs into a, o from git_hosts where id = 'github';
  if a is distinct from '{"mode": "app", "pat": {"tokenEnc": null}, "app": {"appId": "4242", "installationIds": ["99", "100"], "privateKeyEnc": "v1:mirror-check-update"}}'::jsonb
     or o is distinct from '{}'::text[] then
    raise exception 'after updating github_config, the github row holds auth % and orgs %', a, o;
  end if;
  -- The row's own trigger ran inside this one (depth 2) and must not have
  -- written the setting back over what was just written.
  select value into got from app_settings where key = 'github_config';
  if got is distinct from '{"mode": "app", "pat": {"tokenEnc": null}, "app": {"appId": "4242", "installationIds": ["99", "100"], "privateKeyEnc": "v1:mirror-check-update"}, "repoCreationOrgs": []}'::jsonb then
    raise exception 'the setting write did not stick: github_config is now %', got;
  end if;
end $$;

-- 5 · Removing the row empties the setting; a setting write makes it again.
delete from git_hosts where id = 'github';
do $$
declare
  got jsonb;
begin
  select value into got from app_settings where key = 'github_config';
  if got is distinct from '{}'::jsonb then
    raise exception 'after removing the github row, github_config is %, want {}', coalesce(got::text, 'absent');
  end if;
end $$;

insert into app_settings (key, value)
values ('github_config', '{"mode": "pat", "pat": {"tokenEnc": "v1:mirror-check-back"}, "app": {"appId": "", "installationIds": [], "privateKeyEnc": null}, "repoCreationOrgs": ["mirror-check-d"]}')
on conflict (key) do update set value = excluded.value, updated_at = now();
do $$
declare
  r record;
begin
  select * into r from git_hosts where id = 'github';
  if not found then
    raise exception 'a github_config write after the row was removed did not make it again';
  end if;
  if r.kind <> 'github' or r.name <> 'GitHub' or r.base_url <> 'https://github.com'
     or r.auth is distinct from '{"mode": "pat", "pat": {"tokenEnc": "v1:mirror-check-back"}, "app": {"appId": "", "installationIds": [], "privateKeyEnc": null}}'::jsonb
     or r.repo_creation_orgs is distinct from '{mirror-check-d}'::text[] then
    raise exception 'the github row made from the setting is %', row_to_json(r);
  end if;
end $$;

rollback;
\echo 'git-hosts mirror: the defaults, the copy-back and both triggers hold'
