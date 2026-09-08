export function compactGitIdentity(identity: string): string {
  const dirty = identity.endsWith('-dirty');
  const sha = dirty ? identity.slice(0, -'-dirty'.length) : identity;
  const compact = sha.slice(0, 12);
  return dirty ? `${compact}-dirty` : compact;
}
