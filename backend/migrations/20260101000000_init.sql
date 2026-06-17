-- =========================================================
-- G2_67_Jr — Initial placeholder migration (T5-T7).
-- Domain tables, RLS policies, and the tenants/users/datasets
-- schema are added by later tasks (T8+). This file exists so
-- that `sqlx::migrate!()` has at least one entry to embed at
-- compile time and to apply at startup.
-- =========================================================

-- Reserved: RLS scaffolding helper already exists in
-- infra/postgres/init.sql (gmrag_current_tenant()).
-- Domain tables intentionally deferred.
SELECT 1;
