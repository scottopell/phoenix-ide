export function installSeedDiagnostic() {
  const stages = ['before-arm', 'after-arm', 'after-emit', 'failed'];
  const reasons = new Set(['not attached', 'attached; not armed', 'detached', 'provider unavailable',
    'live identity unavailable', 'live connection unavailable', 'event continuity unavailable; phase must be armed again',
    'identity changed; phase must be armed again', 'no armed phase', 'target connection changed; phase must be armed again',
    'reduced state does not match target', 'previous phase invalidated', 'malformed target',
    'invalid actual connection identity', 'expected connection does not match live state',
    'target must require fresh post-arm consumption', 'armed; awaiting fresh reduced state', 'phase ended', 'observer disposed']);
  const keys = ['active', 'notifications', 'indexed', 'atomPresent', 'identityMatches', 'connectionState',
    'connectionEpoch', 'incarnationPresent', 'eventGap', 'eventSequence', 'tokenSequence', 'phaseType',
    'armed', 'watermarkPresent', 'targetEpochMatches', 'targetIncarnationMatches', 'requestMatches',
    'textMatches', 'targetEventSequence', 'targetTokenSequence'];
  window.__touchSeedDiagnostic = [];
  window.__touchCapture = (stage, bridge) => {
    if (!stages.includes(stage)) throw new Error('Unknown diagnostic stage');
    if (stage === 'before-arm') window.__touchSeedDiagnostic = [];
    if (window.__touchSeedDiagnostic.length >= 4) return;
    const receipt = bridge?.status().consumption;
    const d = bridge?.diagnostic();
    const observer = d?.observer ? Object.fromEntries(keys.map(key => [key, d.observer[key] ?? null])) : null;
    window.__touchSeedDiagnostic.push({ stage, receipt: {
      status: receipt?.status === 'OBSERVED' ? 'OBSERVED' : 'UNOBSERVED',
      phase: receipt?.phase === 'seed' ? 'seed' : receipt?.phase == null ? null : 'other',
      reason: receipt?.status === 'UNOBSERVED' ? reasons.has(receipt.reason) ? receipt.reason : 'unrecognized reason' : null,
    }, transport: d ? { initialized: d.initialized, sourceState: d.sourceState,
      tokenListeners: d.tokenListeners, emitted: d.emitted } : null, observer });
  };
}
