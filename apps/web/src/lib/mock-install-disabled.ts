import type { mockInstalls as RealMockInstalls } from "@/lib/mock-install";

/**
 * Production stand-in for the browser install manager (#237). Aliased over
 * `@/lib/mock-install` for non-mock builds so `data/demo.ts` never reaches the
 * bundle. All call sites are guarded by `USING_MOCKS`, which is `false` there;
 * the proxy exists only so `installBridge` keeps its type.
 */
type MockInstalls = typeof RealMockInstalls;

const unavailable = () =>
  Promise.reject(new Error("The mock install manager is not part of a production build."));

export const mockInstalls = new Proxy({} as MockInstalls, {
  get: () => unavailable,
});
