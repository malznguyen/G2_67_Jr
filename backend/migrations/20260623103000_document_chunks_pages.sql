-- =========================================================
-- T84D Phase 3.1 — document_chunks page metadata + hydration index.
--
-- Adds `page_start` / `page_end` to `document_chunks` so the chat
-- citation SSE can carry page numbers (powers the T85 frontend
-- "Jump to page" UX). Both are nullable so pre-T84D rows stay valid.
--
-- Also adds an index on `qdrant_point_id` (P2 scalability fix): the
-- retrieval hydration hot path looks chunks up by point id, and the
-- pre-T84D schema had no index on that column → full table scan per
-- hit.
-- =========================================================

ALTER TABLE document_chunks
    ADD COLUMN page_start INT NULL,
    ADD COLUMN page_end   INT NULL;

CREATE INDEX idx_document_chunks_point ON document_chunks (qdrant_point_id);