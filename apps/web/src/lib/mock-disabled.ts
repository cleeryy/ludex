import type { mockApi as RealMockApi } from "@/lib/mock";

/**
 * Production stand-in for the mock API adapter (#237). `vite.config.ts` aliases
 * `@/lib/mock` here when the build is not a mock/demo build. Every reference to
 * `mockApi` in `lib/api.ts` sits inside a `USING_MOCKS` branch that folds to
 * `false`, so none of these methods can be reached; the proxy only keeps the
 * module shape type-safe without importing the 4 155-line adapter and its demo
 * library.
 */
type MockApi = typeof RealMockApi;

const unavailable = () =>
  Promise.reject(new Error("The mock API is not part of a production build."));

export const mockApi = new Proxy({} as MockApi, {
  get: () => unavailable,
});
