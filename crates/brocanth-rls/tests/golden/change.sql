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
ALTER ROLE "app_member" WITH NOLOGIN INHERIT CREATEDB NOCREATEROLE;
-- create role app_staff
DO $rls$ BEGIN IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'app_staff') THEN CREATE ROLE "app_staff" NOLOGIN; END IF; END $rls$;
ALTER ROLE "app_staff" WITH NOLOGIN NOINHERIT NOCREATEDB NOCREATEROLE;
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
DROP ROLE IF EXISTS "app_staff";
-- alter role app_member
ALTER ROLE "app_member" WITH NOLOGIN INHERIT NOCREATEDB NOCREATEROLE;
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
