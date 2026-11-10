/**
 * Runtime flags in their own module so both the API adapter and the mock
 * install manager can read them without a circular import.
 */
const flag = import.meta.env.VITE_USE_MOCKS as string | undefined;

export const USING_MOCKS = flag === "true" || (flag === undefined && import.meta.env.DEV);
