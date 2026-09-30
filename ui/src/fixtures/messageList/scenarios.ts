import type { Message } from '../../api';
import type { MessageListFixtureData, MessageListScenario } from './types';

const baseMessages: Message[] = [
  {
    message_id: 'user-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Please summarize the plan.' },
    display_data: {},
  },
  {
    message_id: 'agent-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{ type: 'text', text: 'I checked the code and the first pass is complete.\n\nThe result is ready to review.' }],
    display_data: {},
  },
  {
    message_id: 'user-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 3,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:02:00.000Z',
    content: { text: 'Anything else?' },
    display_data: {},
  },
  {
    message_id: 'agent-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 4,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:03:00.000Z',
    content: [{ type: 'text', text: 'Done — I shipped the update and the final summary is here.\n\nYou can proceed with the next step.' }],
    display_data: {},
  },
];
export const compactChronologyInitialMessages: Message[] = [
  {
    message_id: 'chronology-user-1',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: ['Run the compact chronology sequence.', ...Array.from({ length: 300 }, (_, index) => `Reader setup line ${String(index + 1).padStart(2, '0')}`)].join('\n') },
    display_data: {},
  },
  {
    message_id: 'chronology-agent-a',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [
      { type: 'tool_use', id: 'chronology-tool-a', name: 'read_file', input: { path: 'older-a.md' } },
    ],
    display_data: {},
  },
  {
    message_id: 'chronology-result-a',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 3,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:30.000Z',
    content: { tool_use_id: 'chronology-tool-a', content: 'Older A result\n'.repeat(20), is_error: false },
    display_data: {},
  },
];

export const compactChronologyAppendMessages: Message[] = [
  {
    message_id: 'chronology-agent-bc',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 4,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:02:00.000Z',
    content: [
      { type: 'tool_use', id: 'chronology-tool-b', name: 'search', input: { pattern: 'newer B' } },
      { type: 'tool_use', id: 'chronology-tool-c', name: 'bash', input: { op: 'run', cmd: 'echo newer C' } },
    ],
    display_data: {},
  },
];

export const compactChronologyCompletionMessages: Message[] = [
  {
    message_id: 'chronology-result-b',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 5,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:03:30.000Z',
    content: { tool_use_id: 'chronology-tool-b', content: 'B_OK', is_error: false },
    display_data: {},
  },
  {
    message_id: 'chronology-result-c',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 6,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:04:00.000Z',
    content: { tool_use_id: 'chronology-tool-c', content: JSON.stringify({ status: 'exited', exit_code: 0, lines: [{ offset: 0, bytes: 'C_OK' }] }), is_error: false },
    display_data: {},
  },
];

export const compactChronologyFinalMessages: Message[] = [
  {
    message_id: 'chronology-agent-final',
    conversation_id: 'fixture-message-list-compact-chronology',
    sequence_id: 7,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:05:00.000Z',
    content: [{ type: 'text', text: 'Final prose after B and C completed. The latest message must remain reachable without losing the older expanded A detail.' }],
    display_data: {},
  },
];

const toolStripMessages: Message[] = [
  {
    message_id: 'user-tool-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Find where compact tool rendering is implemented and inspect the relevant files.' },
    display_data: {},
  },
  {
    message_id: 'agent-tool-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [
      { type: 'text', text: 'I am checking the targeted surface first.' },
      { type: 'tool_use', id: 'tool-think', name: 'think', input: { thoughts: 'Verify compact density without expanding every historical tool detail.' } },
      { type: 'tool_use', id: 'tool-search-1', name: 'search', input: { pattern: 'CompactToolStrip|deriveToolStripItems', path: 'ui/src', include: '*.tsx' } },
      { type: 'tool_use', id: 'tool-read-1', name: 'read_file', input: { path: 'ui/src/components/MessageComponents.tsx', offset: 811, limit: 80 } },
      { type: 'tool_use', id: 'tool-search-2', name: 'search', input: { pattern: 'compact-tool', path: 'ui/src', include: '*.css' } },
      { type: 'tool_use', id: 'tool-read-2', name: 'read_file', input: { path: 'ui/src/components/agentTurnToolStrip.ts', offset: 1, limit: 120 } },
      { type: 'tool_use', id: 'tool-bash', name: 'bash', input: { op: 'run', cmd: 'pnpm vitest run src/components/agentTurnToolStrip.test.ts' }, display: 'pnpm vitest run src/components/agentTurnToolStrip.test.ts' },
      { type: 'tool_use', id: 'tool-patch', name: 'patch', input: { path: 'ui/src/components/MessageComponents.tsx', patches: [{ operation: 'replace' }, { operation: 'insert_after' }] } },
      { type: 'text', text: 'The compact cards show what each repeated tool did without expanding the full details.' },
    ],
    display_data: {},
  },
  {
    message_id: 'tool-result-bash',
    conversation_id: 'fixture-message-list',
    sequence_id: 3,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:30.000Z',
    content: { tool_use_id: 'tool-search-1', content: 'ui/src/components/MessageComponents.tsx:822:function CompactToolStripImpl\nui/src/components/agentTurnToolStrip.ts:32:export function deriveToolStripItems', is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-read-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 4,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:40.000Z',
    content: { tool_use_id: 'tool-read-1', content: Array.from({ length: 80 }, (_, i) => `${i + 811}\tcompact tool rendering line`).join('\n'), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-search-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 5,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:50.000Z',
    content: { tool_use_id: 'tool-search-2', content: 'ui/src/index.css:321:.compact-tool-strip {\nui/src/index.css:334:.compact-tool-card {\nui/src/index.css:387:.compact-tool-card-summary {', is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-read-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 6,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:00.000Z',
    content: { tool_use_id: 'tool-read-2', content: Array.from({ length: 120 }, (_, i) => `${i + 1}\texport const compactSummaryFixture = true;`).join('\n'), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-bash',
    conversation_id: 'fixture-message-list',
    sequence_id: 7,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:10.000Z',
    content: { tool_use_id: 'tool-bash', content: JSON.stringify({ status: 'exited', exit_code: 0, lines: [] }), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-patch',
    conversation_id: 'fixture-message-list',
    sequence_id: 8,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:20.000Z',
    content: { tool_use_id: 'tool-patch', content: 'Applied patch', is_error: false },
    display_data: {},
  },
  {
    message_id: 'user-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 9,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:03:00.000Z',
    content: { text: 'Great. What changed?' },
    display_data: {},
  },
  {
    message_id: 'agent-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 10,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:04:00.000Z',
    content: [{ type: 'text', text: 'The fixture now shows the compact transcript in a real scroll container.\n\nThe latest assistant summary remains expanded so the end state is visible without an extra click.' }],
    display_data: {},
  },
];

const scrollPolicyMessages: Message[] = Array.from({ length: 80 }, (_, index) => {
  const sequenceId = index + 1;
  const isUser = index % 2 === 0;
  return {
    message_id: `scroll-policy-${sequenceId}`,
    conversation_id: 'fixture-message-list-scroll-policy',
    sequence_id: sequenceId,
    type: isUser ? 'user' : 'agent',
    message_type: isUser ? 'user' : 'agent',
    created_at: new Date(Date.UTC(2025, 0, 1, 10, index)).toISOString(),
    content: isUser
      ? { text: `Scroll policy checkpoint ${sequenceId}: keep this historical item stable.` }
      : [{
          type: 'text',
          text: `Checkpoint ${sequenceId} is complete.\n\n${'Measured conversation output remains deterministic. '.repeat(6)}`,
        }],
    display_data: {},
  } as Message;
});

const markdownImageMessages: Message[] = [
  {
    message_id: 'user-image-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Please include the screenshot preview in your summary.' },
    display_data: {},
  },
  {
    message_id: 'agent-image-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{
      type: 'text',
      text: [
        'Here is the Markdown screenshot preview using the same syntax agents paste into conversations:',
        '',
        '![file-tree-dark-single-slot](/qa/message-list/markdown-image-fixture.svg)',
        '',
        'The image is constrained to the message column and keeps its aspect ratio.',
      ].join('\n'),
    }],
    display_data: {},
  },
];

export const mobileTablePreviewMarkdown = [
  '**Historical fixture — Global status at 11:01 UTC.**',
  '',
  '**Good—review capacity is no longer the gate.** I’ve instructed the coordinator to request one fresh exact-head review where needed, without duplicate requests.',
  '',
  '### Fresh status at 11:01 UTC',
  '',
  '| Stream | Current position |',
  '|---|---|',
  '| **Polish #777** | `b502c97`: **all 7 hosted checks green, 279 threads / 0 unresolved**. Seven-fix batch published; needs refreshed final review. **17008 starts immediately after merge.** |',
  '| **Restart RCA #817** | `bcaae33`: **all applicable hosted checks green**. A causal restart-recovery fix and regressions are published—not provider-blocked anymore. Final review is next. |',
  '| **Provenance #815** | `51b9cd3`: **all 8 hosted checks green**, but three new findings plus one disputed finding still need source-based resolution. Sender-visible message and breadcrumb requirements remain essential. |',
  '',
  '### The coordination problem',
  '**The foreground loop had dropped again.** Its successor went idle around **03:10** after summarizing the handoff instead of continuing execution. Provenance also parked with findings outstanding.',
  '',
  'I explicitly restarted the driving mandate. This time I verified **actual coordinator tools at 11:01**, including delivery of the provenance resume instruction—not merely an active-state flag.',
  '',
  '**Recommended landing order:** qualify the restart fix first to protect upcoming deployments, land #777 and start 17008, and keep provenance fixing its remaining findings in parallel. No new permission is needed for those fixes.',
  '',
  '[Polish #777](https://github.com/scottopell/phoenix-ide/pull/777) · [Restart #817](https://github.com/scottopell/phoenix-ide/pull/817) · [Provenance #815](https://github.com/scottopell/phoenix-ide/pull/815)',
  '',
  '### Stress matrix',
  '',
  '| Label | Long prose at word boundaries |',
  '|---|---|',
  '| **Compact label** | This intentionally long prose should wrap at ordinary word boundaries without collapsing into one-word-per-line text or shrinking the production font. |',
  '| `sha` | `0123456789abcdef0123456789abcdef01234567` remains copyable and may break only as an atomic fallback. |',
  '| URL | https://example.com/releases/mobile-table-preview/very-long-unbroken-path-with-query?revision=0123456789abcdef |',
  '',
  '| ID | State | Detailed outcome |',
  '|---|---|---|',
  '| `#777` | **Green** | Three-column content keeps narrow labels compact while prose receives the available readable width. |',
  '| `#817` | **Review** | Local overflow is acceptable only when readable minimum widths genuinely exceed the viewport. |',
].join('\n');

export interface MobileTablePreviewCase {
  id: string;
  label: string;
  caption: string;
  markdown: string;
  headers: [string, string];
}

export const mobileTablePreviewCases: MobileTablePreviewCase[] = [
  {
    id: 'global-status',
    label: '11:01 status',
    caption: 'Historical fixture — Global status at 11:01 UTC.',
    markdown: mobileTablePreviewMarkdown,
    headers: ['Stream', 'Current position'],
  },
  {
    id: 'source-roles',
    label: 'Source roles',
    caption: 'Historical conversation excerpt · 2026-09-27T19:19:20.898691Z · conversation b2892006-e955-4c27-bfa2-aaab5518222f · message 9993654c-7884-401c-a0d5-6322ab02eb35',
    markdown: '| Source | Appropriate role |\n|---|---|\n| **Phoenix registry** | Models we support, their protocols, capabilities, and orchestration qualification |\n| **Provider catalog** | Discovery and advisory availability information—not an authoritative denial |\n| **Execution response** | Whether that exact model/request is accepted for the current account |',
    headers: ['Source', 'Appropriate role'],
  },
  {
    id: 'candidate-ranking',
    label: 'Ranking',
    caption: 'Historical conversation excerpt · 2026-09-27T20:51:07.621260Z · conversation b2892006-e955-4c27-bfa2-aaab5518222f · message 5e5a3e1d-aa9e-43ed-8775-3d28b868f09b',
    markdown: '| Rank | Candidate | Why / proposed split |\n|---|---|---|\n| **1** | **Deflaking Stage 1** | Already approved; separable work. **Writer A:** bounded-Git and SQLite tests. **Writer B:** scrollback reducer and small UI wiring test. Parent owns integration; keep tmux/browser restructuring out of the first pilot. **Gate:** real write authority must work. |\n| **2** | **Sender identity/provenance** | Highest immediate product payoff after today’s confusing handoffs. Backend persistence/transport and UI presentation can split after agreeing one server-owned contract. Higher integration risk; capability dependency must clear first. |\n| **3** | **API-skill repair** | Smaller, lower-risk pilot: delivered recipe/content versus delivery/contract tests, with parent integration. Useful and overdue, though less substantial as a parallel-efficiency demonstration. |',
    headers: ['Rank', 'Candidate'],
  },
  {
    id: 'landed-changes',
    label: 'Links + SHAs',
    caption: 'Historical conversation excerpt · 2026-09-28T14:36:10.180080Z · conversation b2892006-e955-4c27-bfa2-aaab5518222f · message b9c5efd6-1a2d-4a95-928d-a659c0fdc934',
    markdown: '| Change | What landed |\n|---|---|\n| [#810](https://github.com/scottopell/phoenix-ide/pull/810) — `4fd574ee` | Removed automatic coordinator snapshots to preserve caching; activity is queried on demand. |\n| [#765](https://github.com/scottopell/phoenix-ide/pull/765) — `d6052dbe` | Fixed parent approval capabilities and durable recovery, initially deferring confined Work children. |\n| [#811](https://github.com/scottopell/phoenix-ide/pull/811) — `b487252e` | Restored Work delegation under the **trusted-worker model**, without filesystem sandboxing. |',
    headers: ['Change', 'What landed'],
  },
  {
    id: 'message-matrix',
    label: 'Message matrix',
    caption: 'Historical conversation excerpt · 2026-09-30T02:51:15.332510Z · conversation b2892006-e955-4c27-bfa2-aaab5518222f · message 8121465c-f078-426f-b4c0-c3fc0aab078f',
    markdown: '| Message type | Send to Global when… | Expected response |\n|---|---|---|\n| **Action ready** | A qualified candidate needs merge/deploy, or a concrete intervention is needed | Act, or explain the remaining gate |\n| **Decision needed** | A real scope, authority, risk, or product choice exceeds the owner’s mandate | Decide within delegated authority; ask you only when necessary |\n| **Material change** | Delivery completes, an important assumption becomes false, or a significant blocker appears/disappears | Update the working picture; acknowledge only if useful |\n| **Routine progress** | Tests run, steering is consumed, a normal continuation succeeds, a worker starts another slice | **Roadmap/owner transcript—not a message here** |',
    headers: ['Message type', 'Send to Global when…'],
  },
  {
    id: 'latency-measurements',
    label: 'Latency',
    caption: 'Historical conversation excerpt · 2026-08-06T14:03:10.850474Z · conversation ee7674b6-f572-422b-9eab-2c42a0c29c6b · message 0f213112-67f7-4209-9b84-57fbba6bf7ff',
    markdown: '| Measurement | Median | Range |\n|---|---:|---:|\n| Browser EventSource→init complete | 219 ms | 39–260 ms |\n| Browser init handler | 5 ms | 0.6–6.3 ms |\n| Server init generation | 3 ms | 2–20 ms |',
    headers: ['Measurement', 'Median'],
  },
  {
    id: 'failure-policy',
    label: 'Failure policy',
    caption: 'Historical conversation excerpt · 2026-08-06T14:12:23.403199Z · conversation ee7674b6-f572-422b-9eab-2c42a0c29c6b · message 281f3708-8d8d-424b-97ec-c3ecefbef6e6',
    markdown: '| Data | Failure policy |\n|---|---|\n| Cached conversation list | Ignore failure; fetch server |\n| Cached transcript | Ignore failure; fetch server |\n| Replica metadata | Ignore failure; perform authoritative/full fetch |\n| Cache cleanup after server delete | Ignore/log cache failure; server delete remains successful |\n| Cached sub-agent slug | Ignore failure; use REST fallback |\n| Pending user-message write | Surface failure; never claim it was safely queued |\n| Pending-operation replay | Retry and expose delivery state |',
    headers: ['Data', 'Failure policy'],
  },
  {
    id: 'store-size',
    label: 'Store size',
    caption: 'Historical conversation excerpt · 2026-08-06T15:30:31.990375Z · conversation ee7674b6-f572-422b-9eab-2c42a0c29c6b · message 34b89e6b-1dad-493b-ae8b-f2066e2ff0e8',
    markdown: '| Store | Rows / size |\n|---|---:|\n| Conversation metadata | 382 rows |\n| Cached messages | 132 rows, 729 KiB |\n| Replica metadata | 3 rows |',
    headers: ['Store', 'Rows / size'],
  },
  {
    id: 'model-performance',
    label: '6-column models',
    caption: 'Historical conversation excerpt · 2026-07-19T14:48:33.323993Z · conversation 7d9521ca-160d-40c3-bf95-5c3967f91720 · message aa9fd66e-7dc7-4c0c-b99f-da94b7b42d51',
    markdown: '| Model | Completed | Failed | p50 total | p95 total | Max |\n|---|---:|---:|---:|---:|---:|\n| `gpt-5.4-mini` | 159 | 5 | 2.5 s | 8.9 s | 43.1 s |\n| `gpt-5.4` | 138 | 0 | 4.3 s | 21.1 s | 84.4 s |\n| `gpt-5.6-sol` | 578 | 0 | 5.2 s | 16.2 s | 55.2 s |\n| `gpt-5.6-luna` | 5 | 2 | 19.4 s | 30.2 s | 30.2 s |',
    headers: ['Model', 'Completed'],
  },
];

const wideMarkdownTableMessages: Message[] = [
  {
    message_id: 'user-wide-table-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Compare the operating models in a table.' },
    display_data: {},
  },
  {
    message_id: 'agent-wide-table-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{
      type: 'text',
      text: mobileTablePreviewMarkdown,
    }],
    display_data: {},
  },
];


const continuityParagraphs = Array.from({ length: 28 }, (_, index) => (
  `Continuity marker ${String(index + 1).padStart(2, '0')}: this is deterministic tall-row text used to keep a precise reading position visible while earlier history is inserted.`
));

const prefixContinuityMessages: Message[] = [
  {
    message_id: 'continuity-user-anchor',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 101,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T11:00:00.000Z',
    content: { text: 'Give me a detailed walkthrough with enough depth to read midway through it.' },
    display_data: {},
  },
  {
    message_id: 'continuity-agent-anchor',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 102,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T11:01:00.000Z',
    content: [{ type: 'text', text: continuityParagraphs.join('\n\n') }],
    display_data: {},
  },
  {
    message_id: 'continuity-user-tail',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 103,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T11:02:00.000Z',
    content: { text: 'This tail message keeps the tall response away from the list boundary.' },
    display_data: {},
  },
];

export const prefixContinuityEarlierMessages: Message[] = Array.from({ length: 18 }, (_, index) => ({
  message_id: `continuity-prefix-${index + 1}`,
  conversation_id: 'fixture-message-list-prefix-continuity',
  sequence_id: index + 1,
  type: index % 2 === 0 ? 'user' : 'agent',
  message_type: index % 2 === 0 ? 'user' : 'agent',
  created_at: `2025-01-01T10:${String(index).padStart(2, '0')}:00.000Z`,
  content: index % 2 === 0
    ? { text: `Earlier user message ${index + 1}` }
    : [{ type: 'text', text: `Earlier assistant response ${index + 1}. `.repeat(8) }],
  display_data: {},
} as Message));

export const messageListScenarios = [
  {
    id: 'compact-latest-expanded',
    title: 'Compact latest expanded',
    description: 'Latest finalized assistant summary stays expanded in compact density.',
    theme: 'dark',
  },
  {
    id: 'compact-tool-strip',
    title: 'Compact tool summaries',
    description: 'Compact density collapses repeated tool detail into scannable summary cards.',
    theme: 'dark',
  },
  {
    id: 'compact-expanded-tool-chronology',
    title: 'Compact expanded-tool chronology',
    description: 'Interactive compact-mode sequence: expand older A, append B/C, complete, then final prose.',
    theme: 'dark',
  },
  {
    id: 'scroll-policy-long',
    title: 'Scroll policy long conversation',
    description: 'Long deterministic conversation with controls for real VirtualTranscript tail-follow QA.',
    theme: 'dark',
  },
  {
    id: 'prefix-continuity-offset-bug',
    title: 'Prefix continuity offset bug',
    description: 'Interactive real-VirtualTranscript reproduction of identity-only restoration jumping within a tall row.',
    theme: 'dark',
  },
  {
    id: 'mobile-table-preview-baseline',
    title: 'Mobile table / current baseline',
    description: 'Deployed table CSS 838439341 (unchanged at fixture baseline 7a42b66db); historical Global 11:01 fixture, final and streaming.',
    theme: 'dark',
  },
  {
    id: 'mobile-table-preview-content-wrap',
    title: 'Mobile table / content-aware wrap',
    description: 'Alternative A: compact labels, wide prose, word-boundary wrapping.',
    theme: 'dark',
  },
  {
    id: 'mobile-table-preview-readable-overflow',
    title: 'Mobile table / readable minimum + local overflow',
    description: 'Alternative B: readable minimum columns with local overflow fallback.',
    theme: 'dark',
  },
  {
    id: 'wide-markdown-table',
    title: 'Wide Markdown table / dark',
    description: 'Wide assistant tables keep continuous row surfaces beyond the prose card in dark theme.',
    theme: 'dark',
  },
  {
    id: 'wide-markdown-table-light',
    title: 'Wide Markdown table / light',
    description: 'Wide assistant tables keep continuous row surfaces beyond the prose card in light theme.',
    theme: 'light',
  },
  {
    id: 'markdown-image-dark',
    title: 'Markdown image',
    description: 'Assistant Markdown image syntax renders an inline screenshot preview.',
    theme: 'dark',
  },
] as const satisfies readonly MessageListScenario[];

export type MessageListScenarioId = (typeof messageListScenarios)[number]['id'];

export function getMessageListScenario(id: MessageListScenarioId): MessageListScenario {
  const scenario = messageListScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown message list scenario: ${id}`);
  return scenario;
}

export function messageListFixtureData(scenario: MessageListScenario): MessageListFixtureData {
  const messages = scenario.id === 'compact-tool-strip'
    ? toolStripMessages
    : scenario.id === 'compact-expanded-tool-chronology'
      ? compactChronologyInitialMessages
      : scenario.id === 'markdown-image-dark'
      ? markdownImageMessages
      : scenario.id.startsWith('mobile-table-preview-')
        || scenario.id === 'wide-markdown-table'
        || scenario.id === 'wide-markdown-table-light'
        ? wideMarkdownTableMessages
        : scenario.id === 'scroll-policy-long'
          ? scrollPolicyMessages
          : scenario.id === 'prefix-continuity-offset-bug'
            ? prefixContinuityMessages
            : baseMessages;
  return {
    conversationId: `fixture-message-list-${scenario.id}`,
    slug: `fixture-message-list-${scenario.id}`,
    theme: scenario.theme,
    messages,
    pendingMessages: [],
    convState: { type: 'idle' },
  };
}
