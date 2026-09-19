import { createContext } from 'react';

export type SvgArtifactAccess = { kind: 'owner' } | { kind: 'share'; token: string };

export const SvgArtifactAccessContext = createContext<SvgArtifactAccess>({ kind: 'owner' });
