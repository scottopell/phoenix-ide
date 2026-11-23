/**
 * UTF-16-budgeted string cuts that never split a surrogate pair.
 *
 * `String.prototype.slice` counts UTF-16 code units, so cutting at an
 * arbitrary length can leave half of an astral character (emoji, many CJK
 * extension characters) behind. A lone surrogate renders as U+FFFD and, once
 * serialized by `JSON.stringify`, is rejected by the server's JSON parser —
 * failing the whole request. Truncate user- or model-visible text through
 * these helpers; the `ts-no-utf16-string-truncation` ast-grep rule enforces it.
 */

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/**
 * The longest prefix of `text` that is at most `maxUnits` UTF-16 code units
 * and does not end between the halves of a surrogate pair.
 */
export function safePrefix(text: string, maxUnits: number): string {
  if (text.length <= maxUnits) return text;
  let end = Math.max(0, Math.floor(maxUnits));
  if (end > 0 && isHighSurrogate(text.charCodeAt(end - 1)) && isLowSurrogate(text.charCodeAt(end))) {
    end -= 1;
  }
  return text.slice(0, end);
}

/**
 * The longest suffix of `text` that is at most `maxUnits` UTF-16 code units
 * and does not start between the halves of a surrogate pair.
 */
export function safeSuffix(text: string, maxUnits: number): string {
  if (text.length <= maxUnits) return text;
  let start = text.length - Math.max(0, Math.floor(maxUnits));
  if (start < text.length && isLowSurrogate(text.charCodeAt(start)) && isHighSurrogate(text.charCodeAt(start - 1))) {
    start += 1;
  }
  return text.slice(start);
}

/**
 * Cap on a source line quoted into review notes or task-approval feedback
 * (REQ-PF-009), in UTF-16 code units.
 */
export const QUOTED_LINE_MAX_UNITS = 200;
