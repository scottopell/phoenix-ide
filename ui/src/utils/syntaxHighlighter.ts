/**
 * Shared syntax-highlighter wrapper.
 *
 * Switches from the full `Prism` build (react-syntax-highlighter's default,
 * which bundles every ~200 languages at ~1MB) to `PrismLight` with a curated
 * set of languages typical for LLM-generated code. Cuts react-syntax-highlighter
 * payload by roughly an order of magnitude (bundle-barrel-imports /
 * bundle-dynamic-imports rule).
 *
 * Any language not registered here falls back to unstyled `<pre>` — which is
 * a visually-tolerable degradation for unusual languages, and the user can
 * still read the code.
 */
import { PrismLight as SyntaxHighlighter, createElement } from 'react-syntax-highlighter';
import type { createElementProps } from 'react-syntax-highlighter';

// Language imports — each is its own ESM file under .../languages/prism/.
// Adding a new one here registers an alias below.
import bash from 'react-syntax-highlighter/dist/esm/languages/prism/bash';
import c from 'react-syntax-highlighter/dist/esm/languages/prism/c';
import cpp from 'react-syntax-highlighter/dist/esm/languages/prism/cpp';
import csharp from 'react-syntax-highlighter/dist/esm/languages/prism/csharp';
import css from 'react-syntax-highlighter/dist/esm/languages/prism/css';
import diff from 'react-syntax-highlighter/dist/esm/languages/prism/diff';
import go from 'react-syntax-highlighter/dist/esm/languages/prism/go';
import graphql from 'react-syntax-highlighter/dist/esm/languages/prism/graphql';
import java from 'react-syntax-highlighter/dist/esm/languages/prism/java';
import javascript from 'react-syntax-highlighter/dist/esm/languages/prism/javascript';
import json from 'react-syntax-highlighter/dist/esm/languages/prism/json';
import jsx from 'react-syntax-highlighter/dist/esm/languages/prism/jsx';
import markdown from 'react-syntax-highlighter/dist/esm/languages/prism/markdown';
import markup from 'react-syntax-highlighter/dist/esm/languages/prism/markup';
import python from 'react-syntax-highlighter/dist/esm/languages/prism/python';
import ruby from 'react-syntax-highlighter/dist/esm/languages/prism/ruby';
import rust from 'react-syntax-highlighter/dist/esm/languages/prism/rust';
import sql from 'react-syntax-highlighter/dist/esm/languages/prism/sql';
import toml from 'react-syntax-highlighter/dist/esm/languages/prism/toml';
import tsx from 'react-syntax-highlighter/dist/esm/languages/prism/tsx';
import typescript from 'react-syntax-highlighter/dist/esm/languages/prism/typescript';
import yaml from 'react-syntax-highlighter/dist/esm/languages/prism/yaml';

// Canonical names + common aliases. Case-insensitive matching happens inside
// the highlighter, so we only register the lowercase forms.
const registrations: Array<[string, Parameters<typeof SyntaxHighlighter.registerLanguage>[1]]> = [
  ['bash', bash], ['sh', bash], ['shell', bash], ['zsh', bash],
  ['c', c],
  ['cpp', cpp], ['c++', cpp], ['cxx', cpp],
  ['csharp', csharp], ['cs', csharp], ['c#', csharp],
  ['css', css],
  ['diff', diff], ['patch', diff],
  ['go', go], ['golang', go],
  ['graphql', graphql], ['gql', graphql],
  ['java', java],
  ['javascript', javascript], ['js', javascript],
  ['json', json],
  ['jsx', jsx],
  ['markdown', markdown], ['md', markdown],
  ['markup', markup], ['html', markup], ['xml', markup], ['svg', markup],
  ['python', python], ['py', python],
  ['ruby', ruby], ['rb', ruby],
  ['rust', rust], ['rs', rust],
  ['sql', sql],
  ['toml', toml],
  ['tsx', tsx],
  ['typescript', typescript], ['ts', typescript],
  ['yaml', yaml], ['yml', yaml],
];

for (const [name, grammar] of registrations) {
  SyntaxHighlighter.registerLanguage(name, grammar);
}

export { SyntaxHighlighter, createElement };
export type { createElementProps };
export { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
