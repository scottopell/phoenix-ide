import type { BashResponse } from '../generated/BashResponse';

type UnregisteredBashResponse = Extract<BashResponse, { status: 'unregistered_exited' | 'unregistered_killed' | 'outcome_unknown' }>;

export const bashLaunchOutcomes: UnregisteredBashResponse[] = (
  ['identity_unavailable', 'registration_failed'] as const
).flatMap((launch_error): UnregisteredBashResponse[] => {
  const run = {
    cmd: 'side-effecting-command', label: null, launch_error,
    descendants: 'unverified' as const, output_complete: false, duration_ms: 12,
    start_offset: 1, end_offset: 1, truncated_before: false,
    lines: [{ offset: 1, bytes: 'retained-tail' }],
  };
  return [
    { ...run, status: 'unregistered_exited', exit_code: 4, signal_number: null },
    { ...run, status: 'unregistered_killed', exit_code: null, signal_number: 9 },
    { ...run, status: 'outcome_unknown', error: 'launch_outcome_unknown',
      error_message: 'The command may still be running. Do not automatically retry.' },
  ];
});
