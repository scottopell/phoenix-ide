import { useEffect, useState } from 'react';
import { api, ApiResponseError, type ProductConversationSnapshotView } from '../api';
import {
  getProductConversationSnapshotChangeSequence,
  productConversationSnapshotChangedSince,
  subscribeProductConversationSnapshotChanged,
} from '../notifications';

export function useDesktopRouteOwner(productConversationId: string | null, routeSlug: string | null, search: string) {
  const [productSnapshot, setProductSnapshot] = useState<{
    ownerId: string;
    snapshot: ProductConversationSnapshotView;
    changeSequence: number;
  } | null>(null);
  const [productNotFound, setProductNotFound] = useState<string | null>(null);
  const [validatedPin, setValidatedPin] = useState<{ owner: string; selector: string; id: string } | null>(null);
  const [productSnapshotRetry, setProductSnapshotRetry] = useState(0);
  const requestedPins = new URLSearchParams(search).getAll('source_transcript');
  const requestedPin = requestedPins.length === 1 && requestedPins[0] ? requestedPins[0] : null;
  useEffect(() => {
    if (!productConversationId) {
      setProductSnapshot(null);
      return;
    }
    let cancelled = false;
    let retryTimeout: number | undefined;
    const load = async () => {
      const snapshotChangeSequence = getProductConversationSnapshotChangeSequence();
      const publishSnapshot = (snapshot: ProductConversationSnapshotView) => {
        if (cancelled) return;
        setProductSnapshot({ ownerId: productConversationId, snapshot, changeSequence: snapshotChangeSequence });
      };
      try {
        const snapshot = await api.getProductConversationSnapshot(productConversationId, { message_limit: 1 });
        if (requestedPin) {
          let selected: ProductConversationSnapshotView;
          try {
            selected = await api.getProductConversationSnapshot(requestedPin, { message_limit: 1 });
          } catch (error: unknown) {
            if (error instanceof ApiResponseError && error.status === 404) {
              if (!cancelled) {
                setValidatedPin(null);
                publishSnapshot(snapshot);
              }
              return;
            }
            throw error;
          }
          if (!cancelled) {
            const valid = selected.product_conversation_id === snapshot.product_conversation_id
              && selected.requested_transcript_row_id === requestedPin;
            setValidatedPin(valid ? { owner: productConversationId, selector: requestedPin, id: requestedPin } : null);
          }
        } else if (!cancelled) {
          setValidatedPin(null);
        }
        publishSnapshot(snapshot);
      } catch (error: unknown) {
        if (cancelled) return;
        if (error instanceof ApiResponseError && error.status === 404) {
          setProductNotFound(productConversationId);
          setValidatedPin(null);
          setProductSnapshot(null);
        } else {
          retryTimeout = window.setTimeout(() => setProductSnapshotRetry((value) => value + 1), 2_000);
        }
      }
    };
    void load();
    return () => {
      cancelled = true;
      if (retryTimeout !== undefined) window.clearTimeout(retryTimeout);
    };
  }, [productConversationId, productSnapshotRetry, requestedPin, search]);
  const ownedProductSnapshot = productSnapshot?.ownerId === productConversationId ? productSnapshot.snapshot : null;
  const hasPin = requestedPins.length > 0;
  useEffect(() => {
    const identities = new Set([
      productConversationId,
      productSnapshot?.snapshot.product_conversation_id,
    ].filter((identity): identity is string => Boolean(identity)));
    let refreshed = false;
    const refresh = () => {
      if (refreshed) return;
      refreshed = true;
      setProductSnapshotRetry((value) => value + 1);
    };
    const unsubscribes = [...identities].map((identity) => subscribeProductConversationSnapshotChanged(identity, refresh));
    const canonicalId = productSnapshot?.snapshot.product_conversation_id;
    if (canonicalId && canonicalId !== productConversationId && productSnapshot
      && productConversationSnapshotChangedSince(canonicalId, productSnapshot.changeSequence)) {
      refresh();
    }
    window.addEventListener('phoenix:automatic-continuation-updated', refresh);
    return () => {
      unsubscribes.forEach((unsubscribe) => unsubscribe());
      window.removeEventListener('phoenix:automatic-continuation-updated', refresh);
    };
  }, [productConversationId, productSnapshot]);
  const directExactMember = routeSlug
    && ownedProductSnapshot?.requested_transcript_row_id === routeSlug
    && routeSlug !== ownedProductSnapshot.product_conversation_id
    ? routeSlug
    : null;
  const activeSlug = hasPin
    ? (validatedPin?.owner === productConversationId && validatedPin.selector === requestedPin ? validatedPin.id : null)
    : directExactMember ?? ownedProductSnapshot?.latest_transcript_row_id ?? routeSlug;
  useEffect(() => {
    if (!productConversationId || ownedProductSnapshot || productNotFound === productConversationId) return;
    const retry = () => setProductSnapshotRetry((value) => value + 1);
    const timeout = window.setTimeout(retry, 1_000);
    window.addEventListener('online', retry);
    return () => {
      window.clearTimeout(timeout);
      window.removeEventListener('online', retry);
    };
  }, [ownedProductSnapshot, productConversationId, productSnapshotRetry, productNotFound]);
  return activeSlug;
}
