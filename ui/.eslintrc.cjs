module.exports = {
  root: true,
  env: { browser: true, es2020: true },
  extends: [
    'eslint:recommended',
    'plugin:@typescript-eslint/recommended',
    'plugin:react-hooks/recommended',
  ],
  ignorePatterns: ['dist', '.eslintrc.cjs'],
  parser: '@typescript-eslint/parser',
  plugins: ['react-refresh'],
  rules: {
    'react-refresh/only-export-components': [
      'warn',
      { allowConstantExport: true },
    ],
    // Upgrade from warn to error - missing/unstable deps cause infinite loops
    'react-hooks/exhaustive-deps': 'error',
    // Rules of hooks catches hooks in non-component functions
    'react-hooks/rules-of-hooks': 'error',
  },
  overrides: [
    {
      // Layering guard (task 60002): conversation/ is a foundational
      // store/data layer. components/ sits above it and may import from
      // conversation/, never the reverse — an inverted import means a
      // shared helper is mis-located. Move it to a neutral home (e.g.
      // src/storage/) instead of reaching down from conversation/.
      files: ['src/conversation/**/*.{ts,tsx}'],
      rules: {
        'no-restricted-imports': [
          'error',
          {
            patterns: [
              {
                group: ['**/components/**'],
                message:
                  'conversation/ must not import from components/ — that inverts the layering. Move the shared code to a neutral location such as src/storage/.',
              },
            ],
          },
        ],
      },
    },
  ],
}
