-- create role app_member
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_member') THEN CREATE ROLE "app_member" NOLOGIN; END IF; END $rls$;
ALTER ROLE "app_member" WITH NOLOGIN INHERIT NOCREATEDB NOCREATEROLE;
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
