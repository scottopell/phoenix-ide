import { useEffect, useState } from 'react';
import { api, ApiResponseError, type ProductConversationSnapshotView } from '../api';
import { subscribeProductConversationSnapshotChanged } from '../notifications';

export function useDesktopRouteOwner(productConversationId: string | null, routeSlug: string | null, search: string) {
  const [productSnapshot, setProductSnapshot] = useState<{ ownerId: string; snapshot: ProductConversationSnapshotView } | null>(null);
  const [productNotFound, setProductNotFound] = useState<string | null>(null);
  const [validatedPin, setValidatedPin] = useState<{ owner: string; query: string; id: string } | null>(null);
  const [productSnapshotRetry, setProductSnapshotRetry] = useState(0);
  useEffect(() => {
    if (!productConversationId) {
      setProductSnapshot(null);
      return;
    }
    let cancelled = false;
    setValidatedPin(null);
    api.getProductConversationSnapshot(productConversationId, { message_limit: 1 })
      .then(async (snapshot) => {
        const pins = new URLSearchParams(search).getAll('source_transcript');
        if (pins.length === 1 && pins[0]) {
          const selected = await api.getProductConversationSnapshot(pins[0], { message_limit: 1 });
          if (selected.product_conversation_id === snapshot.product_conversation_id && selected.requested_transcript_row_id === pins[0] && !cancelled) {
            setValidatedPin({ owner: productConversationId, query: search, id: pins[0] });
          }
        }
        if (!cancelled) setProductSnapshot({ ownerId: productConversationId, snapshot });
      })
      .catch((error: unknown) => {
        if (!cancelled && error instanceof ApiResponseError && error.status === 404) setProductNotFound(productConversationId);
        if (!cancelled) setProductSnapshot(null);
      });
    return () => { cancelled = true; };
  }, [productConversationId, productSnapshotRetry, search]);
  const ownedProductSnapshot = productSnapshot?.ownerId === productConversationId ? productSnapshot.snapshot : null;
  const hasPin = new URLSearchParams(search).has('source_transcript');
  useEffect(() => {
    const identities = new Set([
      productConversationId,
      productSnapshot?.snapshot.product_conversation_id,
    ].filter((identity): identity is string => Boolean(identity)));
    const refresh = () => setProductSnapshotRetry((value) => value + 1);
    const unsubscribes = [...identities].map((identity) => subscribeProductConversationSnapshotChanged(identity, refresh));
    return () => unsubscribes.forEach((unsubscribe) => unsubscribe());
  }, [productConversationId, productSnapshot?.snapshot.product_conversation_id]);
  const directExactMember = routeSlug
    && ownedProductSnapshot?.requested_transcript_row_id === routeSlug
    && routeSlug !== ownedProductSnapshot.product_conversation_id
    ? routeSlug
    : null;
  const activeSlug = hasPin
    ? (validatedPin?.owner === productConversationId && validatedPin.query === search ? validatedPin.id : null)
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
