export function delegateTouchRefresh(
  Base: typeof EventSource, args: unknown[], base: string,
): EventSource | null {
  const url = new URL(String(args[0]), base);
  if (url.origin !== new URL(base).origin
    || url.pathname !== '/api/product-conversations/events'
    || url.href.includes('?') || url.href.includes('#')
    || url.username !== '' || url.password !== '') return null;
  return Reflect.construct(Base, args) as EventSource;
}
