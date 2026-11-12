-- create role app_member
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member') THEN CREATE ROLE "app_member" NOLOGIN; ELSIF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND (rolsuper OR rolbypassrls)) THEN RAISE EXCEPTION USING MESSAGE = 'the role "app_member" exists with SUPERUSER or BYPASSRLS, so it bypasses every policy' || chr(10) || 'help: use another role name, or ALTER ROLE "app_member" NOSUPERUSER NOBYPASSRLS as a superuser'; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcanlogin <> false) THEN ALTER ROLE "app_member" NOLOGIN; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolinherit <> true) THEN ALTER ROLE "app_member" INHERIT; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreatedb <> false) THEN ALTER ROLE "app_member" NOCREATEDB; END IF; IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member' AND rolcreaterole <> false) THEN ALTER ROLE "app_member" NOCREATEROLE; END IF; EXCEPTION WHEN insufficient_privilege THEN RAISE EXCEPTION USING MESSAGE = 'the migration role cannot set the attributes of the role "app_member": ' || SQLERRM || chr(10) || 'help: the migration role needs CREATEROLE, ADMIN OPTION on a role it did not create, and CREATEDB to change CREATEDB (docs/rls.md §3); or run the ALTER ROLE as a superuser'; END $rls$;
GRANT "app_member" TO "app_login";
-- create function app.current_tenant
CREATE OR REPLACE FUNCTION "app"."current_tenant"() RETURNS bigint LANGUAGE sql STABLE AS $rls$ SELECT CAST(NULLIF(current_setting('brocanth.tenant_id', TRUE), '') AS bigint) $rls$;
-- create table app.notes
ALTER TABLE "app"."notes" ENABLE ROW LEVEL SECURITY;
-- create table app.posts
ALTER TABLE "app"."posts" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "app"."posts" FORCE ROW LEVEL SECURITY;
-- create grant on app.notes to app_member
GRANT SELECT, INSERT, UPDATE, DELETE ON "app"."notes" TO "app_member";
-- create grant on app.posts to app_member
GRANT SELECT, INSERT, UPDATE, DELETE ON "app"."posts" TO "app_member";
-- create policy notes_app_member_delete on app.notes
CREATE POLICY "notes_app_member_delete" ON "app"."notes" AS PERMISSIVE FOR DELETE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_insert on app.notes
CREATE POLICY "notes_app_member_insert" ON "app"."notes" AS PERMISSIVE FOR INSERT TO "app_member" WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_select on app.notes
CREATE POLICY "notes_app_member_select" ON "app"."notes" AS PERMISSIVE FOR SELECT TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_update on app.notes
CREATE POLICY "notes_app_member_update" ON "app"."notes" AS PERMISSIVE FOR UPDATE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint)) WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy posts_tenant on app.posts
CREATE POLICY "posts_tenant" ON "app"."posts" AS PERMISSIVE FOR ALL TO "app_member", "app_owner" USING ("tenant_id" = "app"."current_tenant"()) WITH CHECK ("tenant_id" = "app"."current_tenant"());
