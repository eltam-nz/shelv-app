// @ts-check
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";
import globals from "globals";

export default tseslint.config(
  {
    ignores: ["dist/**", "target/**", "src-tauri/**", "src/types/**"],
  },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  {
    languageOptions: {
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],

      // The frontend holds no filesystem capability and must not acquire one
      // by importing a plugin directly (docs/PLAN.md §4.2, T1). CI enforces
      // the same rule against the Rust capability set.
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@tauri-apps/plugin-fs",
              message:
                "The frontend must not touch the filesystem. Add a command in src-tauri that takes an id, and resolve paths in shelv-core.",
            },
            {
              name: "@tauri-apps/plugin-shell",
              message: "Shelv does not execute external commands.",
            },
          ],
        },
      ],
    },
  },
  {
    // Config files run in Node and are not part of the app's type graph.
    // Build and check scripts run in Node and are outside the app's type
    // graph, so the type-aware rules have nothing to work from.
    files: ["*.config.{js,ts}", "eslint.config.js", "scripts/**/*.{js,mjs}"],
    languageOptions: { globals: globals.node },
    extends: [tseslint.configs.disableTypeChecked],
  },
  prettier,
);
