const RECOVERY_DISCOVERY_BACKOFF_MS = [2000, 5000, 10000, 30000] as const;

export function recoveryDiscoveryDelay(attempt: number, canAdvance: boolean): number {
  if (canAdvance) return RECOVERY_DISCOVERY_BACKOFF_MS[0];
  return RECOVERY_DISCOVERY_BACKOFF_MS[
    Math.min(attempt, RECOVERY_DISCOVERY_BACKOFF_MS.length - 1)
  ]!;
}
