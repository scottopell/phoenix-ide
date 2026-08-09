let recordedAt: number | undefined;
const CORRELATION_WINDOW_MS = 1_000;

export function recordModuleAcquisitionFailure(): void {
  recordedAt = Date.now();
}

export function isModuleAcquisitionFailure(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  if (/dynamically imported module|importing a module script failed|module preload/i.test(message)) {
    return true;
  }
  return recordedAt !== undefined && Date.now() - recordedAt <= CORRELATION_WINDOW_MS;
}

export function clearModuleAcquisitionFailure(): void {
  recordedAt = undefined;
}
