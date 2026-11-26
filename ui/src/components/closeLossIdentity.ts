export function displayLossIdentity(identity: string): string {
  const prefix = 'git_path_bytes_hex_v1:';
  if (!identity.startsWith(prefix)) return identity;

  const hex = identity.slice(prefix.length);
  if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return identity;

  const bytes = new Uint8Array(hex.match(/../g)?.map((pair) => Number.parseInt(pair, 16)) ?? []);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return Array.from(bytes, (byte) => byte >= 0x20 && byte < 0x7f
      ? String.fromCharCode(byte)
      : `\\x${byte.toString(16).padStart(2, '0')}`).join('');
  }
}
