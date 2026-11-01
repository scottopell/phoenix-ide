import { describe, expect, it } from 'vitest';
import { getErrorPresentation } from './errorPresentation';

describe('capacity failure presentation', () => {
  it.each(['context_capacity_conflict', 'planned_context_pressure'] as const)('never retries or resumes an unchanged %s plan', (kind) => {
    expect(getErrorPresentation(kind)).toEqual({ kind, can_auto_retry: false, can_user_resume: false });
  });
});
