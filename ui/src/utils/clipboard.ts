/**
 * Copy text to the clipboard. Uses navigator.clipboard when available,
 * falls back to a synthetic <textarea> + document.execCommand('copy')
 * for insecure contexts (non-localhost HTTP) where navigator.clipboard
 * is undefined.
 *
 * Returns true on success, false if both paths fail.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // navigator.clipboard exists but threw (permission denied, doc not
      // focused, etc.). Fall through to the execCommand path.
    }
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  try {
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    document.body.removeChild(textarea);
  }
}
