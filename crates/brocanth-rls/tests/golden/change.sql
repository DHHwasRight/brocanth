-- Written by `rls make`. Read it before you commit it.
-- migrations/rls.json records the schema; this file is the step to it.

-- drop policy posts_tenant on app.posts
DROP POLICY IF EXISTS "posts_tenant" ON "app"."posts";
-- drop policy notes_app_member_update on app.notes
DROP POLICY IF EXISTS "notes_app_member_update" ON "app"."notes";
-- drop policy notes_app_member_insert on app.notes
DROP POLICY IF EXISTS "notes_app_member_insert" ON "app"."notes";
-- drop policy notes_app_member_delete on app.notes
DROP POLICY IF EXISTS "notes_app_member_delete" ON "app"."notes";
-- alter role app_member
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member') THEN CREATE ROLE "app_member" NOLOGIN CREATEDB; ELSIF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND (rolsuper OR rolbypassrls)) THEN RAISE EXCEPTION USING MESSAGE = 'the role "app_member" exists with SUPERUSER or BYPASSRLS, so it bypasses every policy' || chr(10) || 'help: use another role name, or ALTER ROLE "app_member" NOSUPERUSER NOBYPASSRLS as a superuser'; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcanlogin <> false) THEN ALTER ROLE "app_member" NOLOGIN; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolinherit <> true) THEN ALTER ROLE "app_member" INHERIT; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreatedb <> true) THEN ALTER ROLE "app_member" CREATEDB; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreaterole <> false) THEN ALTER ROLE "app_member" NOCREATEROLE; END IF; EXCEPTION WHEN insufficient_privilege THEN RAISE EXCEPTION USING MESSAGE = 'the migration role cannot set the attributes of the role "app_member": ' || SQLERRM || chr(10) || 'help: the migration role needs CREATEROLE, ADMIN OPTION on a role it did not create, and CREATEDB to change CREATEDB (docs/rls.md §3); or run the ALTER ROLE as a superuser'; END $rls$;
-- create role app_staff
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff') THEN CREATE ROLE "app_staff" NOLOGIN NOINHERIT; ELSIF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff' AND (rolsuper OR rolbypassrls)) THEN RAISE EXCEPTION USING MESSAGE = 'the role "app_staff" exists with SUPERUSER or BYPASSRLS, so it bypasses every policy' || chr(10) || 'help: use another role name, or ALTER ROLE "app_staff" NOSUPERUSER NOBYPASSRLS as a superuser'; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff' AND rolcanlogin <> false) THEN ALTER ROLE "app_staff" NOLOGIN; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff' AND rolinherit <> false) THEN ALTER ROLE "app_staff" NOINHERIT; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff' AND rolcreatedb <> false) THEN ALTER ROLE "app_staff" NOCREATEDB; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff' AND rolcreaterole <> false) THEN ALTER ROLE "app_staff" NOCREATEROLE; END IF; EXCEPTION WHEN insufficient_privilege THEN RAISE EXCEPTION USING MESSAGE = 'the migration role cannot set the attributes of the role "app_staff": ' || SQLERRM || chr(10) || 'help: the migration role needs CREATEROLE, ADMIN OPTION on a role it did not create, and CREATEDB to change CREATEDB (docs/rls.md §3); or run the ALTER ROLE as a superuser'; END $rls$;
-- alter table app.notes
ALTER TABLE "app"."notes" FORCE ROW LEVEL SECURITY;
-- create grant on app.notes to app_staff
GRANT SELECT ON "app"."notes" TO "app_staff";
-- alter policy notes_app_member_select on app.notes
ALTER POLICY "notes_app_member_select" ON "app"."notes" TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint) OR "id" < 0);
-- create policy notes_staff on app.notes
CREATE POLICY "notes_staff" ON "app"."notes" AS PERMISSIVE FOR SELECT TO "app_staff" USING (TRUE);
-- create policy posts_tenant on app.posts
CREATE POLICY "posts_tenant" ON "app"."posts" AS PERMISSIVE FOR SELECT TO "app_member", "app_owner" USING ("tenant_id" = "app"."current_tenant"());

-- down
-- drop policy posts_tenant on app.posts
DROP POLICY IF EXISTS "posts_tenant" ON "app"."posts";
-- drop policy notes_staff on app.notes
DROP POLICY IF EXISTS "notes_staff" ON "app"."notes";
-- drop grant on app.notes to app_staff
REVOKE SELECT ON "app"."notes" FROM "app_staff";
-- drop role app_staff
DO $rls$ BEGIN IF EXISTS (SELECT FROM pg_catalog.pg_shdepend d JOIN pg_catalog.pg_roles r ON d.refclassid = 'pg_catalog.pg_authid'::regclass AND d.refobjid = r.oid WHERE r.rolname = 'app_staff') THEN RAISE NOTICE 'role % kept: objects on this server still depend on it', 'app_staff'; ELSE DROP ROLE IF EXISTS "app_staff"; END IF; END $rls$;
-- alter role app_member
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member') THEN CREATE ROLE "app_member" NOLOGIN; ELSIF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND (rolsuper OR rolbypassrls)) THEN RAISE EXCEPTION USING MESSAGE = 'the role "app_member" exists with SUPERUSER or BYPASSRLS, so it bypasses every policy' || chr(10) || 'help: use another role name, or ALTER ROLE "app_member" NOSUPERUSER NOBYPASSRLS as a superuser'; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcanlogin <> false) THEN ALTER ROLE "app_member" NOLOGIN; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolinherit <> true) THEN ALTER ROLE "app_member" INHERIT; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreatedb <> false) THEN ALTER ROLE "app_member" NOCREATEDB; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreaterole <> false) THEN ALTER ROLE "app_member" NOCREATEROLE; END IF; EXCEPTION WHEN insufficient_privilege THEN RAISE EXCEPTION USING MESSAGE = 'the migration role cannot set the attributes of the role "app_member": ' || SQLERRM || chr(10) || 'help: the migration role needs CREATEROLE, ADMIN OPTION on a role it did not create, and CREATEDB to change CREATEDB (docs/rls.md §3); or run the ALTER ROLE as a superuser'; END $rls$;
-- alter table app.notes
ALTER TABLE "app"."notes" NO FORCE ROW LEVEL SECURITY;
-- create policy notes_app_member_delete on app.notes
CREATE POLICY "notes_app_member_delete" ON "app"."notes" AS PERMISSIVE FOR DELETE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_insert on app.notes
CREATE POLICY "notes_app_member_insert" ON "app"."notes" AS PERMISSIVE FOR INSERT TO "app_member" WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- alter policy notes_app_member_select on app.notes
ALTER POLICY "notes_app_member_select" ON "app"."notes" TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_update on app.notes
CREATE POLICY "notes_app_member_update" ON "app"."notes" AS PERMISSIVE FOR UPDATE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint)) WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy posts_tenant on app.posts
CREATE POLICY "posts_tenant" ON "app"."posts" AS PERMISSIVE FOR ALL TO "app_member", "app_owner" USING ("tenant_id" = "app"."current_tenant"()) WITH CHECK ("tenant_id" = "app"."current_tenant"());
