/**
 * DiffViewer — backwards-compatible shim. Forwards to the new
 * <DiffView/> built on the shared viewer primitives.
 *
 * The standalone `DiffViewer` component will eventually be removed in
 * favor of consumers using <DiffView/> + the ReviewNotesContext
 * directly. For now this preserves the existing import path used by
 * WorkActions.
 */

import { DiffView } from './viewer/DiffView';
import type { CheckoutStatus } from '../api';

interface DiffViewerProps {
  open: boolean;
  comparator: string;
  commitLog: string;
  committedDiff: string;
  committedTruncatedKib?: number | undefined;
  committedSaturated?: boolean | undefined;
  uncommittedDiff: string;
  uncommittedTruncatedKib?: number | undefined;
  uncommittedSaturated?: boolean | undefined;
  checkoutStatus: CheckoutStatus;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
}

export function DiffViewer(props: DiffViewerProps) {
  return <DiffView {...props} />;
}
