-- alter grant on posts to app_user
REVOKE INSERT, UPDATE ON "posts" FROM "app_user";
-- create grant INSERT on columns of posts to app_user
GRANT INSERT ("author_id", "tenant_id", "title") ON "posts" TO "app_user";
-- create grant UPDATE on columns of posts to app_user
GRANT UPDATE ("status", "title") ON "posts" TO "app_user";
