import { taskApprovalScenarioDefinitions } from './types';
import type { TaskApprovalScenario } from './types';

export const taskApprovalFixturePlan = `# Deliverables

Add trimmed fixtures + \`<stem>.bounds.json\` for the missing/approximated cases, sourced from the **full archive** bucket (\`noaa-nexrad-level2\`, complete history since ~1991) so canonical, documented events can be used:

- AP / ducting outbreak (a documented overnight ducting event).
- Synoptic stratiform shield (widespread light rain, low texture).
- Winter clear day (near-zero echo) — the literal "no false positives" case the realtime summer window can't provide.
- Optionally re-source the hail core from a canonical severe event (e.g. a well-known tornadic supercell) once the filename can be pinned.

## Acceptance criteria

1. The fixture generation command is documented and repeatable.
2. Long inline identifiers such as \`s3://noaa-nexrad-level2/2025/05/16/KOUN/KOUN20250516_235959_V06\` never force the approval screen wider than the mobile viewport.
3. The approval action bar keeps **Discard**, **Send Feedback**, **Continue here**, and **Start fresh conversation** visible on mobile.

\`\`\`bash
uv run scripts/build_qc_fixture.py \\
  --source noaa-nexrad-level2 \\
  --event ap-ducting-overnight \\
  --out fixtures/qc/canonical
\`\`\`
`;

export const taskApprovalScenarios: TaskApprovalScenario[] = taskApprovalScenarioDefinitions.map((scenario) => ({
  ...scenario,
}));

export function getTaskApprovalScenario(id: string | null | undefined): TaskApprovalScenario {
  return taskApprovalScenarios.find((scenario) => scenario.id === id)
    ?? taskApprovalScenarios[0]!;
}
