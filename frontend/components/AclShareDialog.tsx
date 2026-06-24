"use client";

// AclShareDialog (T84) — share/revoke UI for a single resource (file or chat
// session), backed by the ReBAC grant endpoints via `lib/acl.ts`.
//
// Only a resource owner may open/use this dialog in practice; the backend
// re-enforces that (owner-only create/revoke), so this component degrades
// gracefully — a forbidden response surfaces as an inline error rather than a
// client-side guarantee.

import { useCallback, useEffect, useId, useState } from "react";
import {
  aclClient,
  type AclClientConfig,
  type Grant,
  type GrantableRelation,
  type PrincipalType,
  type ShareableResource,
} from "@/lib/acl";

export interface AclShareDialogProps {
  open: boolean;
  onClose: () => void;
  config: AclClientConfig;
  resourceType: ShareableResource;
  resourceId: string;
  /** Optional human label for the resource (shown in the heading). */
  resourceLabel?: string;
}

const RELATIONS: GrantableRelation[] = ["viewer", "editor"];
const PRINCIPAL_TYPES: PrincipalType[] = ["user", "workspace"];

export function AclShareDialog({
  open,
  onClose,
  config,
  resourceType,
  resourceId,
  resourceLabel,
}: AclShareDialogProps) {
  const client = aclClient(config);
  const [grants, setGrants] = useState<Grant[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const [principalType, setPrincipalType] = useState<PrincipalType>("user");
  const [principalId, setPrincipalId] = useState("");
  const [relation, setRelation] = useState<GrantableRelation>("viewer");

  const headingId = useId();

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setGrants(await client.list(resourceType, resourceId));
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load shares");
    } finally {
      setLoading(false);
    }
  }, [client, resourceType, resourceId]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const result = await client.list(resourceType, resourceId);
        if (!cancelled) {
          setGrants(result);
          setError(null);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "Failed to load shares");
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, client, resourceType, resourceId]);

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault();
    if (!principalId.trim()) {
      setError("Enter a user or workspace id to share with");
      return;
    }
    setError(null);
    try {
      await client.create({
        resourceType,
        resourceId,
        principalType,
        principalId: principalId.trim(),
        relation,
      });
      setPrincipalId("");
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to add share");
    }
  }

  async function handleRevoke(grantId: string) {
    setBusyId(grantId);
    setError(null);
    try {
      await client.revoke(grantId);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to revoke share");
    } finally {
      setBusyId(null);
    }
  }

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby={headingId}
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-xl bg-white p-6 shadow-xl dark:bg-neutral-900"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-4 flex items-start justify-between">
          <h2 id={headingId} className="text-lg font-semibold tracking-tight">
            Share {resourceLabel ?? resourceType.replace("_", " ")}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded p-1 text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
          >
            ✕
          </button>
        </div>

        {error && (
          <p className="mb-3 rounded-md bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950 dark:text-red-300">
            {error}
          </p>
        )}

        <form onSubmit={handleAdd} className="mb-4 space-y-2">
          <div className="flex gap-2">
            <select
              aria-label="Principal type"
              value={principalType}
              onChange={(e) => setPrincipalType(e.target.value as PrincipalType)}
              className="rounded-md border border-neutral-300 bg-transparent px-2 py-1.5 text-sm dark:border-neutral-700"
            >
              {PRINCIPAL_TYPES.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
            <input
              aria-label="Principal id"
              value={principalId}
              onChange={(e) => setPrincipalId(e.target.value)}
              placeholder={principalType === "user" ? "user id" : "workspace id"}
              className="flex-1 rounded-md border border-neutral-300 bg-transparent px-2 py-1.5 text-sm dark:border-neutral-700"
            />
            <select
              aria-label="Relation"
              value={relation}
              onChange={(e) => setRelation(e.target.value as GrantableRelation)}
              className="rounded-md border border-neutral-300 bg-transparent px-2 py-1.5 text-sm dark:border-neutral-700"
            >
              {RELATIONS.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </div>
          <button
            type="submit"
            className="w-full rounded-md bg-neutral-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-neutral-700 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
          >
            Share
          </button>
        </form>

        <h3 className="mb-2 text-sm font-medium text-neutral-600 dark:text-neutral-400">
          People &amp; groups with access
        </h3>
        {loading ? (
          <p className="text-sm text-neutral-500">Loading…</p>
        ) : grants.length === 0 ? (
          <p className="text-sm text-neutral-500">Not shared with anyone yet.</p>
        ) : (
          <ul className="space-y-1">
            {grants.map((g) => (
              <li
                key={g.id}
                className="flex items-center justify-between rounded-md border border-neutral-200 px-3 py-2 text-sm dark:border-neutral-800"
              >
                <span className="truncate">
                  <span className="font-mono text-xs text-neutral-500">{g.principal_type}</span>{" "}
                  {g.principal_id}
                  <span className="ml-2 rounded bg-neutral-100 px-1.5 py-0.5 text-xs dark:bg-neutral-800">
                    {g.relation}
                  </span>
                </span>
                {g.relation !== "owner" && (
                  <button
                    type="button"
                    onClick={() => void handleRevoke(g.id)}
                    disabled={busyId === g.id}
                    className="ml-2 shrink-0 rounded px-2 py-1 text-xs text-red-600 hover:bg-red-50 disabled:opacity-50 dark:hover:bg-red-950"
                  >
                    {busyId === g.id ? "…" : "Revoke"}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export default AclShareDialog;
