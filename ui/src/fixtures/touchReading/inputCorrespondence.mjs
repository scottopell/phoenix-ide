export function installInputCorrespondence() {
  window.__touchInputCorrespondence = (intent, observed) => {
    const valid = intent && observed && Number.isSafeInteger(intent.ordinal) && intent.ordinal > 0
      && ['touchStart', 'touchMove'].includes(intent.type)
      && [intent.x, intent.y, observed.x, observed.y].every(Number.isFinite);
    const correspondence = valid && intent.x === observed.x && intent.y === observed.y;
    return { status: correspondence ? 'MATCHED' : 'MISMATCH',
      intent: valid ? { ordinal: intent.ordinal, type: intent.type, x: intent.x, y: intent.y } : null,
      observed: observed && Number.isFinite(observed.x) && Number.isFinite(observed.y)
        ? { x: observed.x, y: observed.y } : null };
  };
}
