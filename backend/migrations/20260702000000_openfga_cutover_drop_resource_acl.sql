-- OpenFGA cutover: authorization tuples now live in OpenFGA.
--
-- PostgreSQL remains the source for tenant-scoped metadata and RLS filtering,
-- but resource sharing grants are no longer stored in the polymorphic
-- resource_acl table. Run the OpenFGA backfill before applying this migration
-- in environments that contain existing resource_acl rows.

DROP TABLE IF EXISTS resource_acl;
