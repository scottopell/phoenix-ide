import { describe, expect, it } from 'vitest';
import { ConversationViewJourney } from './conversationViewJourney';

describe('ConversationViewJourney', () => {
  it('separates network delay from transcript-to-enabled-breadcrumb delay', () => {
    let now = 100;
    const journey = new ConversationViewJourney(() => now);
    now = 1100;
    journey.mark('snapshot_received');
    now = 1120;
    journey.mark('meaningful_transcript');
    now = 1200;
    journey.mark('breadcrumb_presented');
    now = 1220;
    journey.mark('breadcrumb_enabled');
    const report = journey.finish('incomplete')!;
    expect(report.breadcrumb_ready_minus_meaningful_ms).toBe(100);
    expect(report.marks.snapshot_received).toBe(1000);
    expect(report.presentation_evidence).toBe('commit_frame_estimate');
  });

  it('keeps reordered, missing and canceled observations without inventing success', () => {
    let now = 0;
    const journey = new ConversationViewJourney(() => now);
    now = 10;
    journey.mark('breadcrumb_enabled');
    journey.mark('breadcrumb_presented');
    now = 50;
    journey.mark('meaningful_transcript');
    expect(journey.finish('canceled')?.breadcrumb_ready_minus_meaningful_ms).toBe(-40);
    expect(journey.finish('failed')).toBeNull();
    const missing = new ConversationViewJourney(() => now).finish('failed')!;
    expect(missing.breadcrumb_ready_minus_meaningful_ms).toBeNull();
    expect(missing.marks.meaningful_transcript).toBeNull();
  });

  it('bounds correlations and counts drops; late marks cannot mutate a finished report', () => {
    const journey = new ConversationViewJourney(() => 0);
    for (let i = 0; i < 100; i++) journey.correlate('request', crypto.randomUUID());
    const report = journey.finish('incomplete')!;
    expect(report.correlations).toHaveLength(16);
    expect(report.dropped_correlations).toBe(84);
    journey.mark('shell');
    expect(report.marks.shell).toBeNull();
  });

  it('censors saturated timelines instead of presenting a fabricated zero readiness gap', () => {
    let now = 0;
    const journey = new ConversationViewJourney(() => now);
    journey.visibility(false);
    now = 600_000;
    journey.visibility(true);
    journey.mark('meaningful_transcript');
    now += 100;
    journey.mark('breadcrumb_presented');
    journey.mark('breadcrumb_enabled');
    expect(journey.finish('incomplete')).toMatchObject({
      timeline_saturated: true,
      total_ms: 300_000,
      background_ms: 300_000,
      breadcrumb_ready_minus_meaningful_ms: null,
    });
  });

  it('does not convert missing or superseded anchor commands into successful settlement', () => {
    for (const result of ['target_missing', 'superseded'] as const) {
      const journey = new ConversationViewJourney(() => 0);
      journey.anchor(result);
      expect(journey.finish('ready')).toMatchObject({ outcome: 'incomplete', anchor_result: result, marks: { anchor_settled: null } });
    }
  });

  it('measures background time with the same monotonic clock', () => {
    let now = 0;
    const journey = new ConversationViewJourney(() => now);
    now = 5;
    journey.visibility(false);
    now = 105;
    journey.visibility(true);
    now = 120;
    expect(journey.finish('incomplete')?.background_ms).toBe(100);
  });
});
