/**
 * Runtime flags in their own module so both the API adapter and the mock
 * install manager can read them without a circular import.
 *
 * `__USING_MOCKS__` is injected by Vite as a literal boolean in both the dev
 * server and the production build. Keeping it as a compile-time constant (not
 * an `import.meta.env` expression evaluated at runtime) lets the bundler fold
 * every `USING_MOCKS ? mock : native` branch, so `lib/mock-install.ts` and
 * `data/demo.ts` are dropped from production instead of shipping ~50 KB of
 * demo library (#237). The `typeof` guard keeps direct `tsc`/vitest runs
 * working when no define is injected.
 */
export const USING_MOCKS: boolean =
  typeof __USING_MOCKS__ === "boolean" ? __USING_MOCKS__ : import.meta.env.DEV;
