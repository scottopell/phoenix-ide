export function extractProductRouteReference(pathname: string) {
  const match = pathname.match(/^\/(c|product-conversations)\/([^/]+)\/?$/);
  let reference = match?.[2];
  if (!reference) return null;
  try {
    reference = decodeURIComponent(reference);
  } catch {
    // Keep malformed input available for the ordinary not-found path.
  }
  return {
    productConversationId: reference,
    routeSlug: match?.[1] === 'c' ? reference : null,
  };
}
