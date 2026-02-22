import type { PaletteAction } from '../types';

export function createBuiltInActions(opts: {
  navigate: (path: string) => void;
  archiveCurrent?: (() => void) | undefined;
  currentSlug?: string | null | undefined;
}): PaletteAction[] {
  const actions: PaletteAction[] = [
    {
      id: 'new-conversation',
      title: 'New Conversation',
      category: 'Conversation',
      handler: () => opts.navigate('/'),
    },
    {
      id: 'go-to-list',
      title: 'Go to Conversation List',
      category: 'Navigation',
      handler: () => opts.navigate('/'),
    },
  ];

  if (opts.currentSlug && opts.archiveCurrent) {
    actions.push({
      id: 'archive-current',
      title: 'Archive Current Conversation',
      category: 'Conversation',
      handler: opts.archiveCurrent,
    });
  }

  return actions;
}
