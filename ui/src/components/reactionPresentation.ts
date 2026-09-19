import { createContext, type ComponentType } from 'react';
import type { BubbleProps } from './InlineMessageReaction';

export const ReactionPresentationContext = createContext<ComponentType<BubbleProps> | null>(null);
