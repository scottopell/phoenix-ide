import { useCallback, useEffect, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { ConflictError, api, type ProductConversationListRow } from '../api';
import {
  refreshProductConversationList,
  updateProductConversationRows,
} from '../conversation/productConversationList';
import {
  notifyArchiveCloseConflict,
  notifyProductConversationClosed,
  notifyProductConversationListMayHaveChanged,
  notifyProductConversationSnapshotChanged,
} from '../notifications';
import { renamedProductConversationLocation } from '../components/ConversationList.presentation';

export function projectProductConversationHistory(
  rows: readonly ProductConversationListRow[],
  productConversationId: string,
): ProductConversationListRow[] {
  return rows.map((row) => row.product_conversation_id === productConversationId
    ? { ...row, lifecycle: { state: 'history' }, close_action: null }
    : row);
}

export interface ProductConversationActionOptions {
  /** Called after rename/close succeeds (used by Sidebar to poke legacy list refresh). */
  onAfterMutate?: () => void;
}

export interface ProductConversationActionController {
  openRename: (row: ProductConversationListRow) => void;
  openClose: (row: ProductConversationListRow) => void;
  closeTarget: ProductConversationListRow | null;
  closeSubmitting: boolean;
  closeError: string | undefined;
  confirmClose: () => Promise<void>;
  cancelClose: () => void;
  renameTarget: Pick<ProductConversationListRow, 'product_conversation_id' | 'canonical_route' | 'canonical_root' | 'latest_transcript_row_id' | 'presentation'> | null;
  renameError: string | undefined;
  confirmRename: (name: string) => Promise<void>;
  generateRename: () => Promise<void>;
  cancelRename: () => void;
}

export function useProductConversationActions(
  options: ProductConversationActionOptions = {},
): ProductConversationActionController {
  const { onAfterMutate } = options;
  const navigate = useNavigate();
  const location = useLocation();
  const currentLocation = useRef(location);
  currentLocation.current = location;

  const [closeTarget, setCloseTarget] = useState<ProductConversationListRow | null>(null);
  const closeTargetRef = useRef<ProductConversationListRow | null>(null);
  const [closeSubmittingId, setCloseSubmittingId] = useState<string | null>(null);
  const [closeErrorState, setCloseErrorState] = useState<{ productId: string; message: string } | null>(null);

  const renameSelectionRef = useRef(0);
  const [renameTarget, setRenameTarget] = useState<Pick<ProductConversationListRow, 'product_conversation_id' | 'canonical_route' | 'canonical_root' | 'latest_transcript_row_id' | 'presentation'> | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  useEffect(() => {
    closeTargetRef.current = closeTarget;
  }, [closeTarget]);

  const openRename = useCallback((row: ProductConversationListRow) => {
    renameSelectionRef.current += 1;
    setRenameError(undefined);
    setRenameTarget(row);
  }, []);

  const openClose = useCallback((row: ProductConversationListRow) => {
    setCloseErrorState(null);
    setCloseTarget(row);
  }, []);

  const applyProductName = useCallback((renamed: ProductConversationListRow) => {
    if (!renameTarget) return;
    const latestLocation = currentLocation.current;
    const activeRoute = renamedProductConversationLocation(
      latestLocation.pathname, latestLocation.search, latestLocation.hash, renameTarget, renamed,
    );
    if (activeRoute) navigate(activeRoute, { replace: true, state: latestLocation.state });
    updateProductConversationRows((rows) => rows.map((row) =>
      row.product_conversation_id === renamed.product_conversation_id ? renamed : row));
    notifyProductConversationSnapshotChanged(renamed.product_conversation_id);
    notifyProductConversationListMayHaveChanged();
    setRenameTarget(null);
    setRenameError(undefined);
    onAfterMutate?.();
  }, [navigate, onAfterMutate, renameTarget]);

  const confirmRename = useCallback(async (name: string) => {
    if (!renameTarget) return;
    try {
      applyProductName(await api.renameProductConversation(renameTarget.product_conversation_id, name));
    } catch (error) {
      setRenameError(error instanceof Error ? error.message : 'Failed to rename');
    }
  }, [applyProductName, renameTarget]);

  const generateRename = useCallback(async () => {
    if (!renameTarget) return;
    try {
      applyProductName(await api.regenerateProductConversationName(renameTarget.product_conversation_id));
    } catch (error) {
      setRenameError(error instanceof Error ? error.message : 'Failed to generate name');
      throw error;
    }
  }, [applyProductName, renameTarget]);

  const cancelRename = useCallback(() => {
    renameSelectionRef.current += 1;
    setRenameTarget(null);
    setRenameError(undefined);
  }, []);

  const confirmClose = useCallback(async () => {
    if (!closeTarget) return;
    const productId = closeTarget.product_conversation_id;
    if (closeSubmittingId === productId) return;
    setCloseSubmittingId(productId);
    try {
      await api.closeProductConversation(productId);
      updateProductConversationRows((rows) => projectProductConversationHistory(rows, productId));
      notifyProductConversationClosed(productId);
      setCloseTarget((current) => current?.product_conversation_id === productId ? null : current);
      onAfterMutate?.();
      void refreshProductConversationList();
      notifyProductConversationListMayHaveChanged();
      notifyProductConversationSnapshotChanged(productId);
      setCloseErrorState((current) => current?.productId === productId ? null : current);
      setCloseSubmittingId((current) => current === productId ? null : current);
    } catch (err) {
      if (notifyArchiveCloseConflict(closeTarget.canonical_root.transcript_row_id, err)) {
        if (err instanceof ConflictError && err.detail.error_type === 'close_already_history') {
          updateProductConversationRows((rows) => projectProductConversationHistory(rows, productId));
        }
        const confirmationRoute = closeTarget.canonical_route;
        if (closeTargetRef.current?.product_conversation_id === productId) {
          setCloseTarget(null);
          navigate(confirmationRoute);
        }
      } else {
        setCloseErrorState({
          productId,
          message: err instanceof Error ? err.message : 'Failed to close conversation',
        });
      }
      setCloseSubmittingId((current) => current === productId ? null : current);
      console.error('Failed to close product conversation:', err);
    }
  }, [closeSubmittingId, closeTarget, navigate, onAfterMutate]);

  const cancelClose = useCallback(() => {
    setCloseTarget(null);
    setCloseErrorState(null);
  }, []);

  const closeError = closeErrorState && closeErrorState.productId === closeTarget?.product_conversation_id
    ? closeErrorState.message
    : undefined;
  return {
    openRename,
    openClose,
    closeTarget,
    closeSubmitting: closeSubmittingId === closeTarget?.product_conversation_id,
    closeError,
    confirmClose,
    cancelClose,
    renameTarget,
    renameError,
    confirmRename,
    generateRename,
    cancelRename,
  };
}
