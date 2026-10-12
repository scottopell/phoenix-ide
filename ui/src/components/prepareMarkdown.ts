import type { Nodes, Root } from 'mdast';
import remarkGfm from 'remark-gfm';
import remarkParse from 'remark-parse';
import { unified } from 'unified';
import { gfmOptions, interpretMarkdown } from './remarkDetails';

export type MarkdownMode = 'conversation' | 'standard';
const standardParser = unified().use(remarkParse).use(remarkGfm);
export function prepareMarkdown(source: string, mode: MarkdownMode): Root {
  const tree = mode === 'conversation'
    ? interpretMarkdown(source, gfmOptions).tree
    : standardParser.parse(source);
  // Resolve once in AST form so selected roots retain global, first-wins references.
  const definitions = new Map<string, Extract<Nodes, { type: 'definition' }>>();
  const stack: Nodes[] = [tree];
  while (stack.length) {
    const node = stack.pop()!;
    if (node.type === 'definition' && !definitions.has(node.identifier.toUpperCase())) {
      definitions.set(node.identifier.toUpperCase(), node);
    }
    if ('children' in node) for (let i = node.children.length - 1; i >= 0; i--) stack.push(node.children[i]!);
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

