import type { Nodes, Root } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkParse from 'remark-parse';
import { unified } from 'unified';
import { basicOptions, gfmOptions, interpretMarkdown } from './remarkDetails';

export type MarkdownMode = 'conversation' | 'standard';
const standardBasicParser = unified().use(remarkParse);
const standardGfmParser = unified().use(remarkParse).use(remarkGfm);

/** Cheap pre-filter so plain prose/code skips micromark-extension-gfm overhead. */
export function usesGfmSyntax(text: string): boolean {
  if (text.includes('http://') || text.includes('https://') || text.includes('www.') || text.includes('mailto:') || text.includes('xmpp:')) return true;
  if (text.includes('[^')) return true;
  if (text.includes('[') && /\[[ xX]\]/.test(text)) return true;
  if (text.includes('|') && /(^|\n)[^\n]*\|[^\n]*\n[^\n]*-/.test(text)) return true;
  if (text.includes('~') && (/(?:^|[^~])~[^\s~](?:[^\n~]*[^\s~])?~(?:[^~]|$)/.test(text) || /~~[^\n]+~~/.test(text))) return true;
  if (text.includes('@') && /(^|[^\w.+-])[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/.test(text)) return true;
  return false;
}

export function prepareMarkdown(source: string, mode: MarkdownMode): Root {
  const gfm = usesGfmSyntax(source);
  const definitions = new Map<string, Extract<Nodes, { type: 'definition' }>>();
  let tree: Root;
  if (mode === 'conversation') {
    const interpreted = interpretMarkdown(source, gfm ? gfmOptions : basicOptions);
    tree = interpreted.tree;
    if (interpreted.definitions.length === 0) return tree;
    for (const def of interpreted.definitions) {
      const key = def.identifier.toUpperCase();
      if (!definitions.has(key)) definitions.set(key, def);
    }
  } else {
    tree = (gfm ? standardGfmParser : standardBasicParser).parse(source);
    // Resolve once in AST form so selected roots retain global, first-wins references.
    const stack: Nodes[] = [tree];
    while (stack.length) {
      const node = stack.pop()!;
      if (node.type === 'definition' && !definitions.has(node.identifier.toUpperCase())) {
        definitions.set(node.identifier.toUpperCase(), node);
      }
      if ('children' in node) for (let i = node.children.length - 1; i >= 0; i--) stack.push(node.children[i]!);
    }
    if (definitions.size === 0) return tree;
  }
  const parents: Nodes[] = [tree];
  while (parents.length) {
    const parent = parents.pop()!;
    if (!('children' in parent)) continue;
    for (let i = 0; i < parent.children.length; i++) {
      const node = parent.children[i]!;
      if (node.type === 'linkReference' || node.type === 'imageReference') {
        const definition = definitions.get(node.identifier.toUpperCase());
        if (definition) {
          const replacement = node.type === 'linkReference'
            ? { type: 'link' as const, children: node.children, url: definition.url, title: definition.title, position: node.position }
            : { type: 'image' as const, alt: node.alt, url: definition.url, title: definition.title, position: node.position };
          parent.children[i] = replacement;
        }
      }
      parents.push(node);
    }
  }
  return tree;
}

