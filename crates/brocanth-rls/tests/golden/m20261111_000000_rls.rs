// Written by `rls make`. Read it before you commit it.
// migrations/rls.json records the schema; this file is the step to it.

use brocanth_model::sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261111_000000_rls"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}

const UP: &str = r#"
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
"#;

const DOWN: &str = r#"
-- drop policy posts_tenant on app.posts
DROP POLICY IF EXISTS "posts_tenant" ON "app"."posts";
-- drop policy notes_app_member_update on app.notes
DROP POLICY IF EXISTS "notes_app_member_update" ON "app"."notes";
-- drop policy notes_app_member_select on app.notes
DROP POLICY IF EXISTS "notes_app_member_select" ON "app"."notes";
-- drop policy notes_app_member_insert on app.notes
DROP POLICY IF EXISTS "notes_app_member_insert" ON "app"."notes";
-- drop policy notes_app_member_delete on app.notes
DROP POLICY IF EXISTS "notes_app_member_delete" ON "app"."notes";
-- drop grant on app.posts to app_member
REVOKE SELECT, INSERT, UPDATE, DELETE ON "app"."posts" FROM "app_member";
-- drop grant on app.notes to app_member
REVOKE SELECT, INSERT, UPDATE, DELETE ON "app"."notes" FROM "app_member";
-- drop table app.posts
ALTER TABLE IF EXISTS "app"."posts" NO FORCE ROW LEVEL SECURITY;
ALTER TABLE IF EXISTS "app"."posts" DISABLE ROW LEVEL SECURITY;
-- drop table app.notes
ALTER TABLE IF EXISTS "app"."notes" DISABLE ROW LEVEL SECURITY;
-- drop function app.current_tenant
DROP FUNCTION IF EXISTS "app"."current_tenant"();
-- drop role app_member
DROP ROLE IF EXISTS "app_member";
"#;
