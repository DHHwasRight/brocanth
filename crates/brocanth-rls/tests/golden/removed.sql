-- Written by `rls make`. Read it before you commit it.
-- migrations/rls.json records the schema; this file is the step to it.

SET LOCAL lock_timeout = '5s';
-- drop policy notes_app_member_update on app.notes
DROP POLICY IF EXISTS "notes_app_member_update" ON "app"."notes";
-- drop policy notes_app_member_select on app.notes
DROP POLICY IF EXISTS "notes_app_member_select" ON "app"."notes";
-- drop policy notes_app_member_insert on app.notes
DROP POLICY IF EXISTS "notes_app_member_insert" ON "app"."notes";
-- drop policy notes_app_member_delete on app.notes
DROP POLICY IF EXISTS "notes_app_member_delete" ON "app"."notes";
-- drop grant on app.notes to app_member
REVOKE SELECT, INSERT, UPDATE, DELETE ON "app"."notes" FROM "app_member";

-- down
SET LOCAL lock_timeout = '5s';
-- create table app.notes
ALTER TABLE "app"."notes" ENABLE ROW LEVEL SECURITY;
-- create grant on app.notes to app_member
GRANT SELECT, INSERT, UPDATE, DELETE ON "app"."notes" TO "app_member";
-- create policy notes_app_member_delete on app.notes
CREATE POLICY "notes_app_member_delete" ON "app"."notes" AS PERMISSIVE FOR DELETE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_insert on app.notes
CREATE POLICY "notes_app_member_insert" ON "app"."notes" AS PERMISSIVE FOR INSERT TO "app_member" WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_select on app.notes
CREATE POLICY "notes_app_member_select" ON "app"."notes" AS PERMISSIVE FOR SELECT TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
-- create policy notes_app_member_update on app.notes
CREATE POLICY "notes_app_member_update" ON "app"."notes" AS PERMISSIVE FOR UPDATE TO "app_member" USING ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint)) WITH CHECK ("owner_id" = CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint));
