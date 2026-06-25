/**
 * Active tenant id resolver for the openapi-fetch middleware.
 *
 * Returns `null` until the Zustand tenant store (T72) is bootstrapped. The
 * store registers itself here via {@link setActiveTenantResolver} so the
 * client module does not need to import zustand (avoids a circular import
 * between `lib/api/client` and `lib/store/tenant`).
 */
type TenantResolver = () => string | null;

let resolver: TenantResolver = () => null;

export function setActiveTenantResolver(fn: TenantResolver): void {
  resolver = fn;
}

export function getActiveTenantId(): string | null {
  return resolver();
}
