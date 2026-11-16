import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

/**
 * Flat ESLint config for the web app (#237). Before this file the 15
 * `eslint-disable` comments in `src/` were inert: there was no dependency, no
 * config and no script, so `react-hooks/exhaustive-deps` was never checked.
 *
 * Scope is deliberately narrow — TypeScript recommended plus the two classic
 * hooks rules. The React Compiler rules bundled with the plugin are not
 * enabled yet: they need a dedicated cleanup pass, not a perf lane.
 */
export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "coverage/**",
      "node_modules/**",
      "public/**",
      "src/locales/**",
      "src/data/demo.ts",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser },
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
    },
  },
  {
    files: ["*.ts", "*.js", "scripts/**/*.mjs", "src/**/*.test.{ts,tsx}"],
    languageOptions: { globals: { ...globals.node } },
  },
);
