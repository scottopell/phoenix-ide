export const seed = 1729;
export const cases = ['no-stream', 'forced-scroll-bottom', 'uncompensated-above-height', 'oracle-anchor-invalid', 'stream-chunks', 'stream-burst-reflow'];
export function scheduleFor(name, messageId) {
  if (!cases.includes(name)) throw new Error('Unknown bounded case');
  if (name === 'no-stream') return [];
  let state = seed;
  const random = () => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return state >>> 0; };
  const burst = name !== 'stream-chunks';
  const steps = [0, 90, 180, 300, 700, 790, 1200, 1700, 2100, 2400].map((at, i) => ({
    at, type: 'tokens', chunks: Array.from({ length: burst && i % 3 === 0 ? 4 : 1 }, (_, j) =>
      `Chunk ${i}.${j} seed ${seed} value ${random() % 10000}. ${'Measured streaming words. '.repeat(1 + random() % 3)}\n\n`),
  }));
  if (burst) for (const at of [850, 2400]) steps.push({ at, type: 'reflow', messageId,
    text: `Scheduled reflow ${at}, seed ${seed}.\n\n${'An earlier measured paragraph changes height through a production message_updated event. '.repeat(at === 850 ? 10 : 2)}` });
  if (['forced-scroll-bottom', 'uncompensated-above-height', 'oracle-anchor-invalid'].includes(name)) steps.push({ at: 1000, type: 'control', name });
  steps.push({ at: 2550, type: 'tokens', chunks: ['STREAM_COMPLETE_SEED_1729.\n\n'] });
  if (['forced-scroll-bottom', 'uncompensated-above-height', 'oracle-anchor-invalid'].includes(name)) {
    // Quiesce producers 700ms before injection; no later reflow/token can earn detection.
    return steps.filter(s => (s.type === 'tokens' && s.at <= 300) || s.type === 'control').sort((a, b) => a.at - b.at);
  }
  return steps.sort((a, b) => a.at - b.at);
}
