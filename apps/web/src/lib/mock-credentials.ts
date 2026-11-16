/**
 * Credentials for the in-browser mock adapter. Kept in their own module so a
 * production component (the login screen's demo block) can name the password
 * without importing `lib/mock.ts` and dragging `data/demo.ts` into the bundle
 * (#237).
 */
export const MOCK_PASSWORD = "ludex";
