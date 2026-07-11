import mermaid from 'mermaid';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, render, screen, waitFor, act } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { SubAgentStatus, AgentMessage, UserMessage } from './MessageComponents';
import { FilePathContextMenu } from './FilePathContextMenu';
import { MessageContextMenu, OPEN_MESSAGE_VIEWER_EVENT } from './MessageContextMenu';
import { StreamingMessageView } from './StreamingMessage';
import { api, ConflictError, type ConversationState, type Message, type ForkProposalSummary } from '../api';
import { copyToClipboard } from '../utils/clipboard';
import { ForkProposalsProvider, useForkProposals } from '../contexts/ForkProposalsContext';
import { ForkProposalReview } from './ForkProposalReview';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { buildRenderUnits } from '../conversation/renderUnits';

let mockDensity: 'full' | 'compact' = 'full';

vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn((_id: string, code: string) => Promise.resolve({
      svg: `<svg role="img" aria-label="Rendered Mermaid"><text>${code}</text></svg>`,
      bindFunctions: vi.fn(),
    })),
  },
}));

vi.mock('../hooks/useDensity', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../hooks/useDensity')>();
  return {
    ...actual,
    useDensity: () => ({ density: mockDensity, setDensity: vi.fn() }),
  };
});

vi.mock('../utils/clipboard', () => ({
  copyToClipboard: vi.fn().mockResolvedValue(true),
}));

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getConversation: vi.fn(),
      getConversationSlug: vi.fn(),
      listForkProposals: vi.fn(),
      approveForkProposal: vi.fn(),
      dismissForkProposal: vi.fn(),
      requestChangesForkProposal: vi.fn(),
    },
  };
});

beforeEach(() => {
  mockDensity = 'full';
});

function agentMessage(messageId: string, blocks: unknown[], sequenceId = 1): Message {
  return {
    message_id: messageId,
    sequence_id: sequenceId,
    conversation_id: 'agent-1',
    message_type: 'agent',
    content: blocks as Message['content'],
    display_data: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location-search">{location.search}</div>;
}


function toolMessage(toolUseId: string, content: string, sequenceId = 2): Message {
  return {
    message_id: `tool-${toolUseId}`,
    sequence_id: sequenceId,
    conversation_id: 'agent-1',
    message_type: 'tool',
    content: { tool_use_id: toolUseId, content, is_error: false },
    display_data: null,
    created_at: '2026-01-01T00:00:01Z',
  };
}

function systemMessage(messageId: string, text: string, sequenceId = 2): Message {
  return {
    message_id: messageId,
    sequence_id: sequenceId,
    conversation_id: 'agent-1',
    message_type: 'system',
    content: { text },
    display_data: null,
    created_at: '2026-01-01T00:00:01Z',
  };
}

const baseConversation = {
  id: 'agent-1',
  slug: 'agent-1-slug',
  model: 'claude-haiku-4-5',
  cwd: '/tmp',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  message_count: 2,
  state: { type: 'idle' as const },
  browser_session_active: false,
};

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  readonly url: string;
  readyState = 1;
  private listeners = new Map<string, Array<(event: MessageEvent) => void>>();

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    const existing = this.listeners.get(type) ?? [];
    existing.push(listener);
    this.listeners.set(type, existing);
  }

  close() {
    this.readyState = 2;
  }

  emit(type: string, data: unknown) {
    const event = { data: JSON.stringify(data) } as MessageEvent;
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

function emitInit(source: FakeEventSource, messages: Message[], pendingEvents: unknown[] = [], conversation: unknown = baseConversation) {
  source.emit('init', {
    type: 'init',
    sequence_id: 100,
    conversation,
    messages,
    agent_working: false,
    last_sequence_id: 100,
    presentation_mode: 'idle',
    context_window_size: 0,
    project_name: null,
    pending_anchor_sequence_id: messages.reduce((max, m) => Math.max(max, m.sequence_id), 0),
    pending_events: pendingEvents,
    pending_truncated: false,
  });
}

describe('inline tool timers', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('stops the live elapsed counter when a live-arriving tool result renders after an interleaved system message', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:10Z'));

    const startedAtMs = Date.parse('2026-01-01T00:00:00Z');
    const agent = agentMessage('agent-tool-timer', [{
      type: 'tool_use',
      id: 'tool-read',
      name: 'read_file',
      input: { path: 'README.md' },
    }]);
    agent.display_data = { tool_starts: { 'tool-read': startedAtMs } };

    const initialUnits = buildRenderUnits({
      messages: [agent],
      pendingMessages: [],
      convState: { type: 'idle' },
      streamingHandle: null,
    });
    const initialTurn = initialUnits.historicalUnits.find((u) => u.kind === 'agent_turn');
    if (!initialTurn || initialTurn.kind !== 'agent_turn') throw new Error('missing initial agent turn');

    const { rerender } = render(
      <MemoryRouter>
        <AgentMessage
          message={initialTurn.agent}
          toolResults={initialTurn.toolResultsByUseId}
          onOpenFile={undefined}
          activeToolUseId="tool-read"
        />
      </MemoryRouter>,
    );

    expect(document.querySelector('.tool-block-elapsed')).toHaveTextContent('• 10s');

    const completedResult = toolMessage('tool-read', '# README\nDone', 3);
    completedResult.display_data = { duration_ms: 1234 };
    const liveUnits = buildRenderUnits({
      messages: [
        agent,
        systemMessage('sys-live', 'tool completed', 2),
        completedResult,
      ],
      pendingMessages: [],
      convState: { type: 'idle' },
      streamingHandle: null,
    });
    const liveTurn = liveUnits.historicalUnits.find((u) => u.kind === 'agent_turn');
    if (!liveTurn || liveTurn.kind !== 'agent_turn') throw new Error('missing live agent turn');

    rerender(
      <MemoryRouter>
        <AgentMessage
          message={liveTurn.agent}
          toolResults={liveTurn.toolResultsByUseId}
          onOpenFile={undefined}
          activeToolUseId="tool-read"
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('Malformed read_file output')).toBeInTheDocument();
    expect(document.querySelector('.tool-block-elapsed')).toBeNull();
    expect(screen.getByText('• 1.2s')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });

    expect(document.querySelector('.tool-block-elapsed')).toBeNull();
    expect(screen.getByText('• 1.2s')).toBeInTheDocument();
    expect(screen.queryByText('• 40s')).not.toBeInTheDocument();
  });


  it('only shows live elapsed on the current tool when a later tool has started', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:24Z'));

    const agent = agentMessage('agent-current-tool-only', [
      {
        type: 'tool_use',
        id: 'tool-bash',
        name: 'bash',
        input: { op: 'run', cmd: 'ls -la', wait_seconds: 10 },
      },
      {
        type: 'tool_use',
        id: 'tool-read',
        name: 'read_file',
        input: { path: 'SPEARS.md' },
      },
    ]);
    agent.display_data = {
      tool_starts: {
        'tool-bash': Date.parse('2026-01-01T00:00:00Z'),
        'tool-read': Date.parse('2026-01-01T00:00:13Z'),
      },
    };

    render(
      <MemoryRouter>
        <AgentMessage
          message={agent}
          toolResults={new Map()}
          onOpenFile={undefined}
          activeToolUseId="tool-read"
        />
      </MemoryRouter>,
    );

    const bashCard = document.querySelector('[data-tool-id="tool-bash"]');
    const readCard = document.querySelector('[data-tool-id="tool-read"]');
    expect(bashCard).not.toBeNull();
    expect(readCard).not.toBeNull();
    expect(bashCard?.querySelector('.tool-block-elapsed')).toBeNull();
    expect(readCard?.querySelector('.tool-block-elapsed')).toHaveTextContent('• 11s');
    expect(screen.queryByText('• 24s')).not.toBeInTheDocument();
  });

  it('does not show missing-result warnings for queued tools in the latest active turn', () => {
    const agent = agentMessage('agent-queued-tools', [
      { type: 'tool_use', id: 'tool-read-a', name: 'read_file', input: { path: 'a.md' } },
      { type: 'tool_use', id: 'tool-read-b', name: 'read_file', input: { path: 'b.md' } },
    ]);

    render(
      <MemoryRouter>
        <AgentMessage
          message={agent}
          toolResults={new Map()}
          onOpenFile={undefined}
          activeToolUseId="tool-read-a"
          isLatestAgentMessage={true}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByText('result not received')).not.toBeInTheDocument();
  });

  it('shows a diagnostic when a historical tool_use has no paired result', () => {
    const agent = agentMessage('agent-missing-result', [
      { type: 'tool_use', id: 'tool-read', name: 'read_file', input: { path: 'missing.md' } },
    ]);

    render(
      <MemoryRouter>
        <AgentMessage
          message={agent}
          toolResults={new Map()}
          onOpenFile={undefined}
          isLatestAgentMessage={false}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('result not received')).toBeInTheDocument();
  });

  it('does not leave any screenshot-style multi-tool cards in live elapsed state once their results are visible', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:24Z'));

    const startedAtMs = Date.parse('2026-01-01T00:00:00Z');
    const agent = agentMessage('agent-multi-tool-timer', [
      {
        type: 'text',
        text: 'The sandbox remaps ~, so I will inspect the worktree.',
      },
      {
        type: 'tool_use',
        id: 'tool-bash-list',
        name: 'bash',
        input: { op: 'run', cmd: 'ls -la /Users/scottopell/dev | sort | head -250', wait_seconds: 10 },
      },
      {
        type: 'tool_use',
        id: 'tool-bash-find',
        name: 'bash',
        input: { op: 'run', cmd: "find . -maxdepth 5 -iname '*spear*' -print", wait_seconds: 10 },
      },
      {
        type: 'text',
        text: 'I found the v2 repo files and will inspect SPEARS.md.',
      },
      {
        type: 'tool_use',
        id: 'tool-read-root',
        name: 'read_file',
        input: { path: '/Users/scottopell/dev/spears/SPEARS.md' },
      },
      {
        type: 'tool_use',
        id: 'tool-read-range',
        name: 'read_file',
        input: { path: 'SPEARS.md', offset: 1, limit: 240 },
      },
    ]);
    agent.display_data = {
      tool_starts: {
        'tool-bash-list': startedAtMs,
        'tool-bash-find': startedAtMs,
        'tool-read-root': Date.parse('2026-01-01T00:00:13Z'),
        'tool-read-range': Date.parse('2026-01-01T00:00:13Z'),
      },
    };

    const bashList = toolMessage('tool-bash-list', JSON.stringify({ status: 'exited', exit_code: 0, duration_ms: 2400, lines: [] }), 2);
    bashList.display_data = { duration_ms: 2400 };
    const bashFind = toolMessage('tool-bash-find', JSON.stringify({ status: 'exited', exit_code: 0, duration_ms: 2450, lines: [] }), 3);
    bashFind.display_data = { duration_ms: 2450 };
    const readRoot = toolMessage('tool-read-root', '1 # spEARS\n2\n3 spEARS (Simple Project with Agentic Requirements Syntax)\n4 more\n5 more\n6 more', 4);
    readRoot.display_data = { duration_ms: 1100 };
    const readRange = toolMessage('tool-read-range', 'SPEARS.md:1–240', 5);
    readRange.display_data = { duration_ms: 1120 };

    const units = buildRenderUnits({
      messages: [agent, bashList, bashFind, readRoot, readRange],
      pendingMessages: [],
      convState: {
        type: 'tool_executing',
        current_tool: { id: 'tool-read-range', name: 'read_file', input: { path: 'SPEARS.md', offset: 1, limit: 240 } },
        remaining_tools: [],
      },
      streamingHandle: null,
    });
    const turn = units.historicalUnits.find((u) => u.kind === 'agent_turn');
    if (!turn || turn.kind !== 'agent_turn') throw new Error('missing agent turn');

    render(
      <MemoryRouter>
        <AgentMessage
          message={turn.agent}
          toolResults={turn.toolResultsByUseId}
          onOpenFile={undefined}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('The sandbox remaps ~, so I will inspect the worktree.')).toBeInTheDocument();
    expect(screen.getByText('I found the v2 repo files and will inspect SPEARS.md.')).toBeInTheDocument();
    expect(screen.getByText(/# spEARS/)).toBeInTheDocument();
    expect(screen.getByText(/SPEARS\.md:1–240/)).toBeInTheDocument();
    expect(document.querySelectorAll('.tool-block')).toHaveLength(4);
    expect(document.querySelectorAll('.tool-block-status.success')).toHaveLength(4);
    expect(document.querySelectorAll('.tool-block-duration')).toHaveLength(4);
    expect(document.querySelector('.tool-block-elapsed')).toBeNull();
    expect(screen.queryByText('• 24s')).not.toBeInTheDocument();
    expect(screen.queryByText('• 11s')).not.toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });

    expect(document.querySelector('.tool-block-elapsed')).toBeNull();
    expect(screen.queryByText('• 54s')).not.toBeInTheDocument();
    expect(screen.queryByText('• 41s')).not.toBeInTheDocument();
  });
});

describe('message copy affordances', () => {
  beforeEach(() => {
    vi.mocked(copyToClipboard).mockClear();
  });

  it('copies finalized user messages from the mobile copy button', async () => {
    render(
      <UserMessage
        message={{
          message_id: 'user-copy',
          sequence_id: 1,
          conversation_id: 'agent-1',
          message_type: 'user',
          content: { text: 'Please summarize `src/main.rs`.' },
          display_data: null,
          created_at: '2026-01-01T00:00:00Z',
        }}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy your message' }));

    await waitFor(() => {
      expect(copyToClipboard).toHaveBeenCalledWith('Please summarize `src/main.rs`.');
    });
  });

  it('keeps user message status grouped with metadata instead of copy actions', () => {
    const { container } = render(
      <UserMessage
        message={{
          message_id: 'user-layout',
          sequence_id: 1,
          conversation_id: 'agent-1',
          message_type: 'user',
          content: { text: 'Great, push and open a PR please' },
          display_data: null,
          created_at: '2026-01-01T00:00:00Z',
        }}
      />,
    );

    const meta = container.querySelector('.message-header-meta');
    const actions = container.querySelector('.message-header-actions');

    expect(meta?.querySelector('.message-status.sent')).toBeInTheDocument();
    expect(actions?.querySelector('.message-status.sent')).not.toBeInTheDocument();
    expect(actions?.querySelector('.message-mobile-copy')).toBeInTheDocument();
  });

  it('copies finalized agent text blocks as markdown from the mobile copy button', async () => {
    const message = agentMessage('agent-copy', [
      { type: 'text', text: 'First **markdown** block.' },
      { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'pwd' } },
      { type: 'text', text: 'Second block.' },
    ]);

    render(
      <MemoryRouter>
        <AgentMessage
          message={message}
          toolResults={new Map()}
          onOpenFile={vi.fn()}
        />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy Phoenix message' }));

    await waitFor(() => {
      expect(copyToClipboard).toHaveBeenCalledWith('First **markdown** block.\n\nSecond block.');
    });
  });

  it('does not render a message copy button when extracted markdown is empty', () => {
    render(
      <MemoryRouter>
        <UserMessage
          message={{
            message_id: 'user-attachment-only',
            sequence_id: 1,
            conversation_id: 'agent-1',
            message_type: 'user',
            content: {
              text: '',
              files: [{ original_name: 'notes.txt', size_bytes: 12, stored_path: '/tmp/notes.txt', media_type: 'text/plain' }],
            },
            display_data: null,
            created_at: '2026-01-01T00:00:00Z',
          }}
        />
        <AgentMessage
          message={agentMessage('agent-tool-only', [
            { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'pwd' } },
          ])}
          toolResults={new Map()}
          onOpenFile={vi.fn()}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByRole('button', { name: 'Copy your message' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy Phoenix message' })).not.toBeInTheDocument();
  });

  it('renders continuation message copy controls in a non-overlapping row', () => {
    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-continuation-copy', [
            { type: 'text', text: 'Continuation text starts immediately.' },
          ])}
          toolResults={new Map()}
          onOpenFile={vi.fn()}
          isFirstInTurn={false}
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('button', { name: 'Copy Phoenix message' })).toBeInTheDocument();
    expect(container.querySelector('.message-mobile-copy-row')).toBeInTheDocument();
    expect(container.querySelector('.message-mobile-copy-floating')).not.toBeInTheDocument();
  });

  it('keeps message context-menu markdown copy aligned with the mobile copy value', async () => {
    const message = agentMessage('agent-context-copy', [
      { type: 'text', text: 'Context **markdown**.' },
      { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'pwd' } },
      { type: 'text', text: 'More markdown.' },
    ], 14);

    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={message}
            toolResults={new Map()}
            onOpenFile={vi.fn()}
          />
        </div>
        <MessageContextMenu messages={[message]} />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByText(/Context/), { clientX: 20, clientY: 30 });
    fireEvent.click(screen.getByRole('button', { name: 'Copy as Markdown' }));

    await waitFor(() => {
      expect(copyToClipboard).toHaveBeenCalledWith('Context **markdown**.\n\nMore markdown.');
    });
  });

  it('opens message markdown in the sidepanel from the context menu', () => {
    const message = agentMessage('agent-open-sidepanel', [
      { type: 'text', text: 'Long **proposal**.' },
    ], 15);
    const opened: number[] = [];
    const handler = (event: Event) => {
      opened.push((event as CustomEvent<{ sequenceId: number }>).detail.sequenceId);
    };
    window.addEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
    try {
      render(
        <MemoryRouter>
          <div id="messages">
            <AgentMessage
              message={message}
              toolResults={new Map()}
              onOpenFile={vi.fn()}
            />
          </div>
          <MessageContextMenu messages={[message]} />
        </MemoryRouter>,
      );

      fireEvent.contextMenu(screen.getByText(/Long/), { clientX: 20, clientY: 30 });
      fireEvent.click(screen.getByRole('button', { name: 'Open in sidepanel' }));

      expect(opened).toEqual([15]);
    } finally {
      window.removeEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
    }
  });

  it('does not offer sidepanel open when the message has no markdown text', () => {
    const message = agentMessage('agent-tool-only-menu', [
      { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'pwd' } },
    ], 16);

    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={message}
            toolResults={new Map()}
            onOpenFile={vi.fn()}
          />
        </div>
        <MessageContextMenu messages={[message]} />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByText('$ pwd'), { clientX: 20, clientY: 30 });

    expect(screen.queryByRole('button', { name: 'Open in sidepanel' })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Copy command' }).length).toBeGreaterThan(0);
  });

  it('does not offer sidepanel open when disabled for read-only surfaces', () => {
    const message = agentMessage('agent-read-only-menu', [
      { type: 'text', text: 'Read-only **proposal**.' },
    ], 17);

    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={message}
            toolResults={new Map()}
            onOpenFile={vi.fn()}
          />
        </div>
        <MessageContextMenu messages={[message]} enableMessageSidepanel={false} />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByText(/Read-only/), { clientX: 20, clientY: 30 });

    expect(screen.queryByRole('button', { name: 'Open in sidepanel' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy as Markdown' })).toBeInTheDocument();
  });
});

describe('skill command rendering', () => {
  it('renders slash-command user messages as a flat command chip with normal args', () => {
    render(
      <UserMessage
        message={{
          message_id: 'user-skill',
          sequence_id: 1,
          conversation_id: 'agent-1',
          message_type: 'user',
          content: { text: '/dogfood http://localhost:8042' },
          display_data: null,
          created_at: '2026-01-01T00:00:00Z',
        }}
      />,
    );

    expect(screen.getByText('dogfood')).toHaveClass('skill-command-name');
    expect(screen.getByText('http://localhost:8042')).toHaveClass('skill-command-args');
    expect(document.querySelector('.skill-command-chip')).not.toBeNull();
  });

  it('renders skill tool calls with matching chip, source, and snippet', () => {
    const onOpenFile = vi.fn();
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-skill', [{
            type: 'tool_use',
            id: 'tool-skill',
            name: 'skill',
            input: { skill_name: 'agent-browser', args: 'http://localhost:8042' },
          }])}
          toolResults={new Map([
            ['tool-skill', toolMessage(
              'tool-skill',
              'Base directory for this skill: /Users/test/.claude/skills/agent-browser\n# Browser Automation with agent-browser\nMore content',
            )],
          ])}
          onOpenFile={onOpenFile}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByText('skill invocation')).not.toBeInTheDocument();
    expect(screen.getByText('agent-browser')).toHaveClass('skill-command-name');
    expect(screen.getByText('http://localhost:8042')).toHaveClass('skill-command-args');
    expect(screen.getByText('loaded')).toHaveClass('skill-tool-status');
    const sourceButton = screen.getByRole('button', { name: 'SKILL.md' });
    expect(sourceButton).toHaveClass('skill-source-link');
    fireEvent.click(sourceButton);
    expect(onOpenFile).toHaveBeenCalledWith('/Users/test/.claude/skills/agent-browser/SKILL.md', new Set(), 0);
    expect(screen.getByText('Browser Automation with agent-browser')).toHaveClass('skill-tool-snippet');
  });
});

describe('agent message file paths', () => {
  beforeEach(() => {
    vi.mocked(copyToClipboard).mockClear();
  });

  it('opens a file path on left click', () => {
    const onOpenFile = vi.fn();
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-path-click', [{
            type: 'text',
            text: 'Open src/main.rs for details.',
          }])}
          toolResults={new Map()}
          onOpenFile={onOpenFile}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'src/main.rs' }));

    expect(onOpenFile).toHaveBeenCalledWith('src/main.rs', new Set(), 0);
  });

  it('copies absolute and relative paths from the file path context menu', () => {
    const onOpenFile = vi.fn();
    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={agentMessage('agent-msg-path-menu', [{
              type: 'text',
              text: 'Open `src/main.rs` and /repo/project/ui/src/App.tsx next.',
            }])}
            toolResults={new Map()}
            onOpenFile={onOpenFile}
            filePathRootDir="/repo/project/"
          />
        </div>
        <FilePathContextMenu />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByRole('button', { name: 'src/main.rs' }), {
      clientX: 20,
      clientY: 30,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Copy absolute path' }));
    expect(copyToClipboard).toHaveBeenLastCalledWith('/repo/project/src/main.rs');

    fireEvent.contextMenu(screen.getByRole('button', { name: '/repo/project/ui/src/App.tsx' }), {
      clientX: 20,
      clientY: 30,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Copy relative path' }));
    expect(copyToClipboard).toHaveBeenLastCalledWith('ui/src/App.tsx');
  });

  it('reattaches path handling after the messages scroller is replaced', () => {
    render(<FilePathContextMenu />);

    const oldScroller = document.createElement('div');
    oldScroller.id = 'messages';
    oldScroller.innerHTML = '<span role="button" tabindex="0" class="file-path-link" data-file-path="src/old.ts" data-file-absolute-path="/repo/project/src/old.ts" data-file-relative-path="src/old.ts">src/old.ts</span>';
    document.body.appendChild(oldScroller);

    oldScroller.remove();
    const newScroller = document.createElement('div');
    newScroller.id = 'messages';
    newScroller.innerHTML = '<span role="button" tabindex="0" class="file-path-link" data-file-path="src/new.ts" data-file-absolute-path="/repo/project/src/new.ts" data-file-relative-path="src/new.ts">src/new.ts</span>';
    document.body.appendChild(newScroller);

    fireEvent.contextMenu(screen.getByRole('button', { name: 'src/new.ts' }), {
      clientX: 20,
      clientY: 30,
    });

    expect(screen.getByRole('button', { name: 'Copy absolute path' })).toBeInTheDocument();
    newScroller.remove();
  });

  it('closes the message context menu when opening the file path menu', () => {
    const message = agentMessage('agent-msg-overlap', [{
      type: 'text',
      text: 'Plain text before src/main.rs then.',
    }], 18);

    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={message}
            toolResults={new Map()}
            onOpenFile={vi.fn()}
            filePathRootDir="/repo/project"
          />
        </div>
        <FilePathContextMenu />
        <MessageContextMenu messages={[message]} />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByText(/Plain text before/), {
      clientX: 20,
      clientY: 30,
    });
    expect(screen.getByRole('button', { name: 'Copy as Markdown' })).toBeInTheDocument();

    fireEvent.contextMenu(screen.getByRole('button', { name: 'src/main.rs' }), {
      clientX: 40,
      clientY: 50,
    });

    expect(screen.getByRole('button', { name: 'Copy absolute path' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy as Markdown' })).not.toBeInTheDocument();
  });

  it('keeps the message context menu available for non-path message text', () => {
    const message = agentMessage('agent-msg-context', [{
      type: 'text',
      text: 'Plain message text with src/main.rs nearby.',
    }], 17);

    render(
      <MemoryRouter>
        <div id="messages">
          <AgentMessage
            message={message}
            toolResults={new Map()}
            onOpenFile={vi.fn()}
            filePathRootDir="/repo/project"
          />
        </div>
        <FilePathContextMenu />
        <MessageContextMenu messages={[message]} />
      </MemoryRouter>,
    );

    fireEvent.contextMenu(screen.getByText(/Plain message text/), {
      clientX: 20,
      clientY: 30,
    });

    expect(screen.getByRole('button', { name: 'Copy as Markdown' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Copy absolute path' })).not.toBeInTheDocument();
  });
});


describe('conversation markdown links', () => {
  it('opens finalized agent Markdown links in a new tab with safe rel attributes', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-markdown-link', [{
            type: 'text',
            text: 'Review [PR 123](https://github.com/acme/repo/pull/123) before merging.',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    const link = screen.getByRole('link', { name: 'PR 123' });
    expect(link).toHaveAttribute('href', 'https://github.com/acme/repo/pull/123');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('keeps finalized agent plain URL auto-links opening in a new tab', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-auto-link', [{
            type: 'text',
            text: 'Plain URL: https://github.com/acme/repo/pull/456',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    const link = screen.getByRole('link', { name: 'https://github.com/acme/repo/pull/456' });
    expect(link).toHaveAttribute('href', 'https://github.com/acme/repo/pull/456');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('renders finalized agent local Markdown images through the preview allowlist path', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-local-image', [{
            type: 'text',
            text: 'Preview:\n\n![file-tree-dark-single-slot](ui/qa-artifacts/grounding-panel/file-tree-dark-single-slot.png)',
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    const image = screen.getByRole('img', { name: 'file-tree-dark-single-slot' });
    expect(image).toHaveAttribute(
      'src',
      '/preview/repo/project/ui/qa-artifacts/grounding-panel/file-tree-dark-single-slot.png',
    );
    expect(image).toHaveClass('conversation-markdown-image');
    expect(image).toHaveAttribute('loading', 'lazy');
  });

  it('keeps finalized agent remote Markdown image URLs unchanged', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-remote-image', [{
            type: 'text',
            text: '![remote screenshot](https://example.com/screenshot.png)',
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'remote screenshot' })).toHaveAttribute(
      'src',
      'https://example.com/screenshot.png',
    );
  });

  it('preserves percent-escaped finalized agent local Markdown image paths', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-escaped-image', [{
            type: 'text',
            text: '![escaped screenshot](ui/qa-artifacts/Screenshot%202026.png)',
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'escaped screenshot' })).toHaveAttribute(
      'src',
      '/preview/repo/project/ui/qa-artifacts/Screenshot%202026.png',
    );
  });

  it('keeps already-preview Markdown image URLs unchanged', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-preview-image', [{
            type: 'text',
            text: '![preview screenshot](/preview/repo/project/shot.png)',
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'preview screenshot' })).toHaveAttribute(
      'src',
      '/preview/repo/project/shot.png',
    );
  });

  it('keeps safe data image URLs through ReactMarkdown sanitization', () => {
    const dataUrl = 'data:image/png;base64,iVBORw0KGgo=';
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-data-image', [{
            type: 'text',
            text: `![data screenshot](${dataUrl})`,
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'data screenshot' })).toHaveAttribute('src', dataUrl);
  });

  it('keeps blob image URLs through ReactMarkdown sanitization', () => {
    const blobUrl = 'blob:http://localhost/screenshot-id';
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-blob-image', [{
            type: 'text',
            text: `![blob screenshot](${blobUrl})`,
          }])}
          toolResults={new Map()}
          filePathRootDir="/repo/project"
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'blob screenshot' })).toHaveAttribute('src', blobUrl);
  });

  it('does not leave no-root local Markdown image paths as browser-relative URLs', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-no-root-image', [{
            type: 'text',
            text: '![shared screenshot](ui/qa-artifacts/shared.png)',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('img', { name: 'shared screenshot' })).not.toHaveAttribute('src');
  });

  it('opens streaming agent Markdown links in a new tab with safe rel attributes', async () => {
    render(
      <MemoryRouter>
        <StreamingMessageView buffer={{
          text: 'Streaming [PR 789](https://github.com/acme/repo/pull/789) now.',
          lastSequence: 1,
          startedAt: Date.now(),
          requestId: 'test-req-id',
        }} />
      </MemoryRouter>,
    );

    await waitFor(() => {
      const link = screen.getByRole('link', { name: 'PR 789' });
      expect(link).toHaveAttribute('href', 'https://github.com/acme/repo/pull/789');
      expect(link).toHaveAttribute('target', '_blank');
      expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    });
  });

  it('renders streaming agent local Markdown images through the preview allowlist path', async () => {
    render(
      <MemoryRouter>
        <StreamingMessageView
          buffer={{
            text: 'Streaming image: ![capture](./ui/qa-artifacts/capture.png)',
            lastSequence: 1,
            startedAt: Date.now(),
            requestId: 'test-req-id',
          }}
          rootDir="/repo/project/"
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByRole('img', { name: 'capture' })).toHaveAttribute(
        'src',
        '/preview/repo/project/ui/qa-artifacts/capture.png',
      );
    });
  });
});


describe('markdown table rendering', () => {
  const wideTableMarkdown = [
    '| Alpha | Beta | Gamma | Delta | Epsilon | Zeta |',
    '| --- | --- | --- | --- | --- | --- |',
    '| one | two | three | four | five | six |',
  ].join('\n');

  it('wraps finalized agent message tables in a local horizontal scroll container', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-1', [{ type: 'text', text: wideTableMarkdown }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    const table = screen.getByRole('table');
    const wrapper = table.parentElement;
    expect(wrapper).toHaveClass('markdown-table-scroll');
    expect(wrapper?.parentElement).toHaveClass('agent-text-block');
  });

  it('keeps finalized agent message task lists enabled for plus and ordered markers', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-tasks', [{
            type: 'text',
            text: '+ [ ] plus task\n1. [x] ordered task',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    const checkboxes = screen.getAllByRole('checkbox');
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes[0]).not.toBeChecked();
    expect(checkboxes[1]).toBeChecked();
  });

  it('keeps finalized agent message strikethrough and email autolinks enabled', () => {
    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-inline-gfm', [{
            type: 'text',
            text: 'Contact contact@example.com and ignore ~obsolete~ text.',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByRole('link', { name: 'contact@example.com' })).toHaveAttribute('href', 'mailto:contact@example.com');
    expect(container.querySelector('del')?.textContent).toBe('obsolete');
  });

  it('keeps finalized agent message footnotes enabled when GFM syntax is present', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-footnote', [{
            type: 'text',
            text: 'Phoenix has a note.[^1]\n\n[^1]: Footnote content',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('Footnotes')).toBeInTheDocument();
    expect(screen.getByText('Footnote content')).toBeInTheDocument();
  });

  it('wraps streaming message tables in a local horizontal scroll container', async () => {
    render(
      <MemoryRouter>
        <StreamingMessageView buffer={{ text: wideTableMarkdown, lastSequence: 1, startedAt: Date.now(), requestId: 'test-req-id' }} />
      </MemoryRouter>,
    );

    await waitFor(() => {
      const table = screen.getByRole('table');
      const wrapper = table.parentElement;
      expect(wrapper).toHaveClass('markdown-table-scroll');
      expect(wrapper?.parentElement).toHaveClass('agent-text-block');
    });
  });
});


describe('finalized code fence highlighting', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });


  it('renders mermaid fences as diagrams with source controls', async () => {
    const mermaidSource = 'flowchart TD\n  User[Developer] --> Cmd["./dda.py inv"]';

    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-mermaid', [{
            type: 'text',
            text: `\`\`\`mermaid\n${mermaidSource}\n\`\`\``,
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(await screen.findByRole('img', { name: 'Rendered Mermaid' })).toBeInTheDocument();
    expect(mermaid.render).toHaveBeenCalledWith(expect.stringMatching(/^phoenix-mermaid-/), mermaidSource);
    expect(container.querySelector('code.language-mermaid')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Source' }));

    expect(screen.getByText(/flowchart TD/)).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Copy Mermaid source' }));
    });
    expect(copyToClipboard).toHaveBeenCalledWith(mermaidSource);
  });


  it('renders short one-line assistant prose fully in compact mode', () => {
    mockDensity = 'compact';

    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-short-prose', [{
            type: 'text',
            text: 'Branch protection requires required checks first, so I’ll enable auto-merge for the approved PR and monitor it.',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(container.querySelector('.agent-text-block')).toBeInTheDocument();
    expect(container.querySelector('.agent-text-collapsed')).not.toBeInTheDocument();
  });

  it('collapses one-line assistant prose that exceeds the compact preview limit', () => {
    mockDensity = 'compact';
    const text = 'A'.repeat(150);

    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-long-preview-prose', [{
            type: 'text',
            text,
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(container.querySelector('.agent-text-collapsed')).toBeInTheDocument();
    expect(container.querySelector('.agent-text-block')).not.toBeInTheDocument();
    expect(container.querySelector('.agent-text-collapsed')).toHaveTextContent(`${'A'.repeat(139)}…`);
  });

  it('collapses short multi-line assistant prose when later lines are omitted', () => {
    mockDensity = 'compact';

    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-multiline-prose', [{
            type: 'text',
            text: 'First line summary.\n\nSecond line with more detail.',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(container.querySelector('.agent-text-collapsed')).toBeInTheDocument();
    expect(container.querySelector('.agent-text-block')).not.toBeInTheDocument();
    expect(container.querySelector('.agent-text-collapsed')).toHaveTextContent('First line summary.');
  });

  it('renders compact text fully when forceExpandedText is set', () => {
    mockDensity = 'compact';

    const { container } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-force-expanded', [{
            type: 'text',
            text: 'First line summary.\n\nSecond line with more detail.',
          }])}
          toolResults={new Map()}
          forceExpandedText
        />
      </MemoryRouter>,
    );

    expect(container.querySelector('.agent-text-block')).toBeInTheDocument();
    expect(container.querySelector('.agent-text-collapsed')).not.toBeInTheDocument();
  });

  it('renders short mermaid fences in compact mode instead of collapsing them', async () => {
    mockDensity = 'compact';

    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-short-mermaid', [{
            type: 'text',
            text: '```mermaid\nflowchart TD\n  A --> B\n```',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(await screen.findByTestId('mermaid-diagram')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /```mermaid/ })).not.toBeInTheDocument();
    expect(mermaid.render).toHaveBeenCalledWith(expect.stringMatching(/^phoenix-mermaid-/), 'flowchart TD\n  A --> B');
  });

  it('falls back to source when mermaid rendering fails', async () => {
    vi.mocked(mermaid.render).mockRejectedValueOnce(new Error('Parse error on line 2'));

    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-mermaid-error', [{
            type: 'text',
            text: '```mermaid\nnot a diagram\n```',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(await screen.findByText('Mermaid render failed.')).toBeInTheDocument();
    expect(screen.getByText('Parse error on line 2')).toBeInTheDocument();
    expect(screen.getByText('not a diagram')).toBeInTheDocument();
    expect(screen.queryByRole('img', { name: 'Rendered Mermaid' })).not.toBeInTheDocument();
  });

  it('renders readable code immediately and upgrades highlighting during idle time', async () => {
    let idleCallback: (() => void) | undefined;
    const requestIdleCallback = vi.fn((callback: () => void) => {
      idleCallback = callback;
      return 1;
    });
    const cancelIdleCallback = vi.fn();
    vi.stubGlobal('requestIdleCallback', requestIdleCallback);
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);

    const { container, unmount } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-code', [{
            type: 'text',
            text: '```ts\nconst answer: number = 42;\n```',
          }])}
          toolResults={new Map()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('const answer: number = 42;')).toBeInTheDocument();
    expect(container.querySelector('code.language-ts')).toBeInTheDocument();
    expect(container.querySelector('code.language-ts span.token')).not.toBeInTheDocument();
    expect(requestIdleCallback).toHaveBeenCalledWith(expect.any(Function), { timeout: 1500 });

    await act(async () => {
      idleCallback?.();
    });

    await waitFor(() => {
      expect(container.querySelector('code.language-ts span.token')).toBeInTheDocument();
      expect(screen.getByText('const')).toBeInTheDocument();
    });

    unmount();
  });
});

describe('read_file structured result view', () => {
  it('renders a ranged preview with metadata, copy, and open actions', async () => {
    const onOpenFile = vi.fn();
    const lines = Array.from({ length: 25 }, (_, index) => `${index + 40}\tfixture line ${index + 40}`).join('\n');

    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-read-structured', [
            { type: 'tool_use', id: 'tool-read-structured', name: 'read_file', input: { path: 'src/lib.rs', offset: 40, limit: 25 } },
          ])}
          toolResults={new Map([['tool-read-structured', toolMessage('tool-read-structured', lines)]])}
          onOpenFile={onOpenFile}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('src/lib.rs')).toBeInTheDocument();
    expect(screen.getByText(/25 lines • lines 40-64/)).toBeInTheDocument();
    expect(screen.getByText('requested 25')).toBeInTheDocument();
    expect(screen.getByText('fixture line 40')).toBeInTheDocument();
    expect(screen.queryByText('fixture line 60')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'View full file' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'View full file' }));
    expect(onOpenFile).toHaveBeenCalledWith('src/lib.rs', new Set(), 40, 64);

    fireEvent.click(screen.getByRole('button', { name: /Copy file excerpt/i }));
    await waitFor(() => {
      expect(copyToClipboard).toHaveBeenCalledWith(lines);
    });

    expect(screen.getByText(/5 more returned lines/)).toBeInTheDocument();
    expect(screen.queryByText('fixture line 60')).not.toBeInTheDocument();
  });

  it('renders empty and malformed fallback states', () => {
    const { rerender } = render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-read-empty', [
            { type: 'tool_use', id: 'tool-read-empty', name: 'read_file', input: { path: 'empty.txt' } },
          ])}
          toolResults={new Map([['tool-read-empty', toolMessage('tool-read-empty', '')]])}
          onOpenFile={vi.fn()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('(empty file)')).toBeInTheDocument();
    expect(screen.getByText('No file content returned')).toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-read-malformed', [
            { type: 'tool_use', id: 'tool-read-malformed', name: 'read_file', input: { path: 'broken.txt', offset: 10, limit: 3 } },
          ])}
          toolResults={new Map([['tool-read-malformed', toolMessage('tool-read-malformed', 'oops\n12\tvalid line')]])}
          onOpenFile={vi.fn()}
        />
      </MemoryRouter>,
    );

    expect(screen.getByText('broken.txt')).toBeInTheDocument();
    expect(screen.getByText(/Ignored 1 non-numbered line/)).toBeInTheDocument();
    expect(screen.getByText('valid line')).toBeInTheDocument();
  });
});

describe('compact tool summaries', () => {
  it('renders contextual compact cards and expands to full tool details', () => {
    mockDensity = 'compact';
    const message = agentMessage('agent-compact-tools', [
      { type: 'tool_use', id: 'tool-search', name: 'search', input: { pattern: 'compact|Tool|tool', path: 'ui/src', include: '*.tsx' } },
      { type: 'tool_use', id: 'tool-read', name: 'read_file', input: { path: 'ui/src/components/MessageComponents.tsx', offset: 711, limit: 40 } },
      { type: 'tool_use', id: 'tool-bash', name: 'bash', input: { op: 'run', cmd: './dev.py check' } },
    ]);
    const results = new Map<string, Message>([
      ['tool-search', toolMessage('tool-search', 'a.ts:1: one\nb.ts:2: two')],
      ['tool-read', toolMessage('tool-read', 'line one\nline two')],
      ['tool-bash', toolMessage('tool-bash', JSON.stringify({ status: 'exited', exit_code: 0, lines: [] }))],
    ]);

    const { container } = render(
      <MemoryRouter>
        <AgentMessage message={message} toolResults={results} onOpenFile={undefined} />
      </MemoryRouter>,
    );

    expect(container.querySelectorAll('.compact-tool-card')).toHaveLength(3);
    expect(screen.getByText('2 matches in 2 files')).toBeInTheDocument();
    expect(screen.getByText('2 lines')).toBeInTheDocument();
    expect(screen.getByText('exited 0')).toBeInTheDocument();
    expect(container.querySelectorAll('.tool-block')).toHaveLength(0);

    fireEvent.click(screen.getByRole('button', { name: /read_file: 2 lines .*expand tool detail/i }));

    expect(container.querySelectorAll('.tool-block')).toHaveLength(3);
    expect(screen.getByText('ui/src/components/MessageComponents.tsx:711-750')).toBeInTheDocument();
  });
});

describe('bash tool inspector affordance', () => {
  const bashToolUse = { type: 'tool_use', id: 'tool-bash', name: 'bash', input: { op: 'run', cmd: 'sleep 60' } };

  function bashResult(response: Record<string, unknown>) {
    return toolMessage('tool-bash', JSON.stringify(response));
  }

  it('opens the process inspector for a structured bash result handle', async () => {
    render(
      <MemoryRouter initialEntries={['/c/test-conv']}>
        <ViewerSlotProvider scopeKey="test-conv" browserSessionActive={false}>
          <AgentMessage
            message={agentMessage('agent-msg-bash', [bashToolUse])}
            toolResults={new Map([['tool-bash', bashResult({ status: 'running', handle: 'b-17', lines: [] })]])}
            onOpenFile={undefined}
            workScopeKey="ws-test-123"
          />
          <LocationProbe />
        </ViewerSlotProvider>
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'inspect →' }));

    await waitFor(() => {
      expect(screen.getByTestId('location-search')).toHaveTextContent(
        '?viewer=inspect&scope=ws-test-123&handle=b-17',
      );
    });
  });

  it('does not render inspect without a work scope key', () => {
    render(
      <MemoryRouter>
        <AgentMessage
          message={agentMessage('agent-msg-bash', [bashToolUse])}
          toolResults={new Map([['tool-bash', bashResult({ status: 'running', handle: 'b-17', lines: [] })]])}
          onOpenFile={undefined}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByRole('button', { name: 'inspect →' })).not.toBeInTheDocument();
  });

  it('does not render inspect when the bash response has no handle', () => {
    render(
      <MemoryRouter>
        <ViewerSlotProvider scopeKey="test-conv" browserSessionActive={false}>
          <AgentMessage
            message={agentMessage('agent-msg-bash', [bashToolUse])}
            toolResults={new Map([['tool-bash', bashResult({ status: 'exited', exit_code: 0, lines: [] })]])}
            onOpenFile={undefined}
            workScopeKey="ws-test-123"
          />
        </ViewerSlotProvider>
      </MemoryRouter>,
    );

    expect(screen.queryByRole('button', { name: 'inspect →' })).not.toBeInTheDocument();
  });
});
describe('SubAgentStatus inline activity', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    FakeEventSource.instances = [];
    vi.stubGlobal('EventSource', FakeEventSource);
    (api.getConversationSlug as ReturnType<typeof vi.fn>).mockResolvedValue('agent-1-slug');
  });

  it('shows compact status plus lazy-loaded child tool timeline', async () => {
    const childMessages = [
      agentMessage('agent-msg-1', [
        { type: 'text', text: 'I will inspect the target directory first.' },
        { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'ls /root/var' } },
      ]),
      toolMessage('tool-1', 'cache\nlog\ntmp'),
    ];

    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [],
      completed_results: [{
        agent_id: 'agent-1',
        task: 'Review telescope config',
        outcome: { type: 'success', result: 'Found the config issue.' },
      }],
    };

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    expect(screen.getByText('success')).toBeInTheDocument();
    expect(screen.getByText('activity')).toBeInTheDocument();
    expect(api.getConversation).not.toHaveBeenCalled();

    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: baseConversation,
      messages: childMessages,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    fireEvent.click(screen.getByText(/Review telescope config/));
    await waitFor(() => expect(api.getConversation).toHaveBeenCalledTimes(1));

    expect(await screen.findByText(/I will inspect/)).toBeInTheDocument();
    expect(screen.getByText('bash')).toBeInTheDocument();
    expect(screen.getByText('ls /root/var')).toBeInTheDocument();
    expect(screen.getByText('cache')).toBeInTheDocument();
    expect(screen.getByText('final outcome')).toBeInTheDocument();
    expect(screen.getByText(/Found the config issue/)).toBeInTheDocument();
  });

  it('resolves sub-agent transcript and final-result Markdown images against the child root', async () => {
    const childConversation = {
      ...baseConversation,
      cwd: '/repo/child-worktree',
      worktree_path: '/repo/child-worktree',
    };
    const childMessages = [
      agentMessage('agent-msg-image', [{
        type: 'text',
        text: 'Child transcript image: ![child-shot](ui/qa-artifacts/child.png)',
      }]),
    ];

    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [],
      completed_results: [{
        agent_id: 'agent-1',
        task: 'Capture screenshot evidence',
        outcome: { type: 'success', result: 'Final image: ![final-shot](ui/qa-artifacts/final.png)' },
      }],
    };

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: childConversation,
      messages: childMessages,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });
    fireEvent.click(screen.getByText(/Capture screenshot evidence/));
    await waitFor(() => expect(api.getConversation).toHaveBeenCalledTimes(1));

    expect(await screen.findByRole('img', { name: 'child-shot' })).toHaveAttribute(
      'src',
      '/preview/repo/child-worktree/ui/qa-artifacts/child.png',
    );
    expect(screen.getByRole('img', { name: 'final-shot' })).toHaveAttribute(
      'src',
      '/preview/repo/child-worktree/ui/qa-artifacts/final.png',
    );
  });

  it('resolves streaming sub-agent Markdown images against the child root', async () => {
    const childConversation = {
      ...baseConversation,
      cwd: '/repo/streaming-child',
      worktree_path: '/repo/streaming-child',
      state: { type: 'llm_requesting' as const, attempt: 1 },
    };
    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'agent-1', task: 'Stream screenshot evidence' }],
      completed_results: [],
    };

    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: childConversation,
      messages: [],
      agent_working: true,
      presentation_mode: 'working',
      context_window_size: 0,
    });

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByText(/Stream screenshot evidence/));
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => {
      emitInit(FakeEventSource.instances[0]!, [], [], childConversation);
      FakeEventSource.instances[0]!.emit('token', {
        sequence_id: 101,
        request_id: 'child-req-1',
        text: '![live-shot](ui/qa-artifacts/live.png)',
      });
    });

    await waitFor(() => {
      expect(screen.getByRole('img', { name: 'live-shot' })).toHaveAttribute(
        'src',
        '/preview/repo/streaming-child/ui/qa-artifacts/live.png',
      );
    });
  });

  it('folds child stream init pending events before advancing sequence floor', async () => {
    const initialMessage = agentMessage('agent-msg-1', [
      { type: 'tool_use', id: 'tool-1', name: 'bash', input: { cmd: 'ls /root/var' } },
    ], 2);
    const pendingTool = toolMessage('tool-1', 'cache\nlog\ntmp', 3);

    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'agent-1', task: 'Review telescope config' }],
      completed_results: [],
    };

    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: baseConversation,
      messages: [initialMessage],
      agent_working: true,
      presentation_mode: 'working',
      context_window_size: 0,
    });

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByText(/Review telescope config/));
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => emitInit(FakeEventSource.instances[0]!, [initialMessage], [{
      type: 'message',
      sequence_id: pendingTool.sequence_id,
      message: pendingTool,
    }]));

    expect(await screen.findByText('bash')).toBeInTheDocument();
    expect(screen.getByText('cache')).toBeInTheDocument();
  });

  it('surfaces typed backend errors from child streams', async () => {
    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'agent-1', task: 'Review telescope config' }],
      completed_results: [],
    };

    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: baseConversation,
      messages: [],
      agent_working: true,
      presentation_mode: 'working',
      context_window_size: 0,
    });

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByText(/Review telescope config/));
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => FakeEventSource.instances[0]!.emit('error', {
      type: 'error',
      sequence_id: 2,
      message: 'Child auth expired',
      error: { kind: 'credential' },
    }));

    expect(await screen.findByText('Child auth expired')).toBeInTheDocument();
  });

  it('retries live expansion until the child conversation row exists', async () => {
    vi.useFakeTimers();
    (api.getConversation as ReturnType<typeof vi.fn>)
      .mockRejectedValueOnce(new Error('Conversation not found'))
      .mockResolvedValueOnce({
        conversation: baseConversation,
        messages: [],
        agent_working: true,
        presentation_mode: 'working',
        context_window_size: 0,
      });

    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'agent-1', task: 'Review telescope config' }],
      completed_results: [],
    };

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    try {
      fireEvent.click(screen.getByText(/Review telescope config/));
      await act(async () => { await Promise.resolve(); });
      expect(api.getConversation).toHaveBeenCalledTimes(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });
      expect(FakeEventSource.instances).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('streams each expanded running sub-agent concurrently (no global single-stream cap)', async () => {
    (api.getConversation as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({
        conversation: baseConversation,
        messages: [],
        agent_working: true,
        presentation_mode: 'working',
        context_window_size: 0,
      })
      .mockResolvedValueOnce({
        conversation: { ...baseConversation, id: 'agent-2', slug: 'agent-2-slug' },
        messages: [],
        agent_working: true,
        presentation_mode: 'working',
        context_window_size: 0,
      });

    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [
        { agent_id: 'agent-1', task: 'First running task' },
        { agent_id: 'agent-2', task: 'Second running task' },
      ],
      completed_results: [],
    };

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByText(/First running task/));
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    fireEvent.click(screen.getByText(/Second running task/));

    // The global single-live-stream cap was removed (HTTP/2 multiplexes), so
    // both expanded running sub-agents stream concurrently with no error.
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(2));
    expect(screen.queryByText(/Another live sub-agent stream/)).not.toBeInTheDocument();
  });

  it('preserves expanded state when a pending sub-agent completes', async () => {
    (api.getConversation as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      conversation: baseConversation,
      messages: [],
      agent_working: true,
      presentation_mode: 'working',
      context_window_size: 0,
    }).mockResolvedValueOnce({
      conversation: baseConversation,
      messages: [],
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 0,
    });

    const pendingState: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'agent-1', task: 'Review telescope config' }],
      completed_results: [],
    };
    const completedState: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [],
      completed_results: [{
        agent_id: 'agent-1',
        task: 'Review telescope config',
        outcome: { type: 'success', result: 'Done without collapsing.' },
      }],
    };

    const { rerender } = render(
      <MemoryRouter>
        <SubAgentStatus stateData={pendingState} />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByText(/Review telescope config/));
    await waitFor(() => expect(screen.getByRole('button', { expanded: true })).toBeInTheDocument());

    rerender(
      <MemoryRouter>
        <SubAgentStatus stateData={completedState} />
      </MemoryRouter>,
    );

    expect(screen.getByRole('button', { expanded: true })).toBeInTheDocument();
    expect(await screen.findByText('final outcome')).toBeInTheDocument();
    expect(screen.getByText(/Done without collapsing/)).toBeInTheDocument();
  });

  it('renders timeout as a distinct state', async () => {
    const state: ConversationState = {
      type: 'awaiting_sub_agents',
      pending: [],
      completed_results: [{
        agent_id: 'agent-1',
        task: 'Slow task',
        outcome: { type: 'timed_out' },
      }],
    };

    render(
      <MemoryRouter>
        <SubAgentStatus stateData={state} />
      </MemoryRouter>,
    );

    expect(screen.getByText('timed out')).toBeInTheDocument();
    expect(screen.getByText(/exceeded its time limit/)).toBeInTheDocument();
  });
});

describe('fork proposal Review affordance (REQ-PROJ-034 / 037)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function forkToolResult(proposalId: string): Message {
    return {
      message_id: 'tool-fork',
      sequence_id: 2,
      conversation_id: 'agent-1',
      message_type: 'tool',
      content: {
        tool_use_id: 'tool-fork',
        content: 'Fork proposal recorded; pending review.',
        is_error: false,
      },
      // The proposal id rides the tool-result display_data (REQ-PROJ-034).
      display_data: { fork_proposal_id: proposalId },
      created_at: '2026-01-01T00:00:01Z',
    };
  }

  function proposal(overrides: Partial<ForkProposalSummary>): ForkProposalSummary {
    return {
      id: 'prop-1',
      status: 'pending',
      title: 'Fix the parser bug',
      priority: 'p2',
      task_file: 'tasks/00042-p2-ready--fix-parser.md',
      body: '# Fix the parser bug\n\nThe tokenizer drops trailing commas.',
      ...overrides,
    };
  }

  function renderTranscript(proposals: ForkProposalSummary[]) {
    (api.listForkProposals as ReturnType<typeof vi.fn>).mockResolvedValue(proposals);
    const message = agentMessage('agent-msg-fork', [
      { type: 'tool_use', id: 'tool-fork', name: 'propose_task', input: { task_file: 'tasks/00042-p2-ready--fix-parser.md' } },
    ]);
    return render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1">
          <AgentMessage
            message={message}
            toolResults={new Map([['tool-fork', forkToolResult('prop-1')]])}
            onOpenFile={undefined}
          />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );
  }

  it('shows a Review button while the proposal is pending', async () => {
    renderTranscript([proposal({ status: 'pending' })]);
    expect(await screen.findByRole('button', { name: 'Review' })).toBeInTheDocument();
  });

  it('withdraws the Review button and shows a terminal status once spawned', async () => {
    renderTranscript([
      proposal({ status: 'spawned', fork_conversation_id: 'fork-conv-9' }),
    ]);
    // Terminal status replaces the Review affordance.
    expect(await screen.findByRole('button', { name: 'Forked' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  });

  it('shows a dismissed terminal status with no Review action', async () => {
    renderTranscript([proposal({ status: 'dismissed' })]);
    expect(await screen.findByText('Dismissed')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  });

  it('renders fenced mermaid diagrams in proposal bodies', async () => {
    const { container } = render(
      <ForkProposalReview
        proposal={proposal({
          status: 'pending',
          body: ['# Fix the parser bug', '', '```mermaid', 'flowchart TD', '  A --> B', '```'].join('\n'),
        })}
        onApprove={vi.fn()}
        onDismiss={vi.fn()}
        onRequestChanges={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    expect(await screen.findByTestId('mermaid-diagram')).toBeInTheDocument();
    expect(container.querySelector('code.language-mermaid')).not.toBeInTheDocument();
  });

  // Bug 1: a non-conflict action failure must leave the modal interactive so the
  // user can retry or Escape out, rather than stranding it permanently busy.
  // The provider catches the failure and resolves its action promise WITHOUT
  // closing the modal (no onOutcome → ForkReviewOverlay stays mounted); the
  // modal must still clear `busy`.
  it('re-enables the review modal after a failed action settles (so Escape/retry work)', async () => {
    // Models the provider's caught-error path: promise resolves, modal not unmounted.
    const onApprove = vi.fn().mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(
      <ForkProposalReview
        proposal={proposal({ status: 'pending' })}
        onApprove={onApprove}
        onDismiss={vi.fn()}
        onRequestChanges={vi.fn()}
        onClose={onClose}
      />,
    );

    const approveBtn = screen.getByRole('button', { name: /Approve/ });
    await act(async () => {
      fireEvent.click(approveBtn);
    });

    // The action rejected; the modal must not be stuck disabled.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Dismiss' })).not.toBeDisabled();
      expect(screen.getByRole('button', { name: /Approve/ })).not.toBeDisabled();
    });

    // Escape is honored again now that busy is cleared.
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
  });

  // Bug 2: a proposal id that arrives after the initial list fetch (live
  // conversation) must trigger a refetch so its Review affordance appears.
  it('refetches for an id missing from the initial list, then renders Review', async () => {
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // First fetch returns no proposals; the streamed id isn't known yet.
    listMock.mockResolvedValueOnce([]);
    // The triggered refetch now learns about the proposal.
    listMock.mockResolvedValueOnce([proposal({ status: 'pending' })]);

    const message = agentMessage('agent-msg-fork', [
      { type: 'tool_use', id: 'tool-fork', name: 'propose_task', input: { task_file: 'tasks/00042-p2-ready--fix-parser.md' } },
    ]);
    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1">
          <AgentMessage
            message={message}
            toolResults={new Map([['tool-fork', forkToolResult('prop-1')]])}
            onOpenFile={undefined}
          />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    // After the second (refetch) fetch lands, the Review affordance shows.
    expect(await screen.findByRole('button', { name: 'Review' })).toBeInTheDocument();
    expect(listMock).toHaveBeenCalledTimes(2);
  });

  // Bug 3: a dismiss that returns no_op (resolved in another tab) must NOT show a
  // local "dismissed" terminal state; it refetches and surfaces already_resolved.
  it('honors a no_op dismiss: no local dismissed state, refetches true status', async () => {
    const onOutcome = vi.fn();
    (api.dismissForkProposal as ReturnType<typeof vi.fn>).mockResolvedValue({
      success: true,
      no_op: true,
    });
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // Initial list: pending. Post-dismiss refetch: the real resolution (spawned).
    listMock.mockResolvedValueOnce([proposal({ status: 'pending' })]);
    listMock.mockResolvedValueOnce([
      proposal({ status: 'spawned', fork_conversation_id: 'fork-conv-9' }),
    ]);

    function DismissHarness() {
      const fork = useForkProposals();
      if (!fork) return null;
      return (
        <button type="button" onClick={() => void fork.dismiss('prop-1')}>
          trigger-dismiss
        </button>
      );
    }

    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1" onOutcome={onOutcome}>
          <DismissHarness />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    // Wait for the initial list fetch to settle.
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(1));

    await act(async () => {
      fireEvent.click(screen.getByText('trigger-dismiss'));
    });

    await waitFor(() => {
      expect(onOutcome).toHaveBeenCalledWith({ kind: 'already_resolved' });
    });
    // A no_op dismiss never reports a 'dismissed' outcome...
    expect(onOutcome).not.toHaveBeenCalledWith({ kind: 'dismissed' });
    // ...and it refetches to reconcile the true status.
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));
  });

  // Bug N4: when the origin conversation reaches a terminal state the backend
  // retires its pending proposals to `dismissed`. The provider must refetch once
  // on that transition so the Review affordance withdraws without a reload.
  it('refetches and withdraws the Review affordance when the origin goes terminal', async () => {
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // Initial list: pending (Review visible). Post-terminal refetch: retired.
    listMock.mockResolvedValueOnce([proposal({ status: 'pending' })]);
    listMock.mockResolvedValueOnce([proposal({ status: 'dismissed' })]);

    const message = agentMessage('agent-msg-fork', [
      { type: 'tool_use', id: 'tool-fork', name: 'propose_task', input: { task_file: 'tasks/00042-p2-ready--fix-parser.md' } },
    ]);

    function Harness({ terminal }: { terminal: boolean }) {
      return (
        <MemoryRouter>
          <ForkProposalsProvider conversationId="agent-1" originTerminal={terminal}>
            <AgentMessage
              message={message}
              toolResults={new Map([['tool-fork', forkToolResult('prop-1')]])}
              onOpenFile={undefined}
            />
          </ForkProposalsProvider>
        </MemoryRouter>
      );
    }

    const { rerender } = render(<Harness terminal={false} />);

    // Pending: Review button shows.
    expect(await screen.findByRole('button', { name: 'Review' })).toBeInTheDocument();
    expect(listMock).toHaveBeenCalledTimes(1);

    // Origin transitions to terminal → the false→true edge triggers a refetch.
    await act(async () => {
      rerender(<Harness terminal={true} />);
    });

    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));
    // The affordance withdraws: terminal status, no Review action.
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
    });
    expect(screen.getByText('Dismissed')).toBeInTheDocument();
  });

  // Bug P2 (race-proof withdrawal): the terminal `state_change` can arrive before
  // the backend retires the pending proposals, so the immediate refetch may still
  // read a `pending` row. The affordance must NOT offer Review when the origin is
  // terminal — it withdraws to a "No longer available" state regardless of the
  // stored status, because approve/request-changes would 409.
  it('withdraws Review for a pending proposal whose origin is terminal (store still reports pending)', async () => {
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // The backend hasn't retired yet: every fetch (initial + terminal refetch +
    // retry) reads the still-pending row. The UI must withdraw regardless.
    listMock.mockResolvedValue([proposal({ status: 'pending' })]);

    const message = agentMessage('agent-msg-fork', [
      { type: 'tool_use', id: 'tool-fork', name: 'propose_task', input: { task_file: 'tasks/00042-p2-ready--fix-parser.md' } },
    ]);

    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1" originTerminal={true}>
          <AgentMessage
            message={message}
            toolResults={new Map([['tool-fork', forkToolResult('prop-1')]])}
            onOpenFile={undefined}
          />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    // Withdrawn state shows; no Review action despite the store reporting pending.
    expect(await screen.findByText('No longer available')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  });

  // Bug P2 (resolved proposals survive): a proposal already resolved
  // (spawned/promoted) before the origin went terminal keeps its real terminal
  // status + link — withdrawal applies only to still-`pending` proposals.
  it('keeps a spawned terminal status (and link) when the origin is terminal', async () => {
    (api.listForkProposals as ReturnType<typeof vi.fn>).mockResolvedValue([
      proposal({ status: 'spawned', fork_conversation_id: 'fork-conv-9' }),
    ]);

    const message = agentMessage('agent-msg-fork', [
      { type: 'tool_use', id: 'tool-fork', name: 'propose_task', input: { task_file: 'tasks/00042-p2-ready--fix-parser.md' } },
    ]);

    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1" originTerminal={true}>
          <AgentMessage
            message={message}
            toolResults={new Map([['tool-fork', forkToolResult('prop-1')]])}
            onOpenFile={undefined}
          />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    // The Forked link survives a terminal origin; it is not withdrawn.
    expect(await screen.findByRole('button', { name: 'Forked' })).toBeInTheDocument();
    expect(screen.queryByText('No longer available')).not.toBeInTheDocument();
  });

  // Bug N5: a 409 from approve is ambiguous. If the post-conflict refetch shows
  // the proposal STILL pending, the conflict was an actionable precondition
  // failure (e.g. a branch collision) — surface its message, do NOT claim the
  // proposal was already resolved.
  it('surfaces the conflict message when an approve 409 leaves the proposal pending', async () => {
    const onOutcome = vi.fn();
    const onError = vi.fn();
    (api.approveForkProposal as ReturnType<typeof vi.fn>).mockRejectedValue(
      new ConflictError({
        error: 'Branch tasks/00042 already exists outside this fork',
        error_type: 'branch_collision',
      }),
    );
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // Initial + post-conflict refetch both show the proposal still pending.
    listMock.mockResolvedValue([proposal({ status: 'pending' })]);

    function ApproveHarness() {
      const fork = useForkProposals();
      if (!fork) return null;
      return (
        <button type="button" onClick={() => void fork.approve('prop-1')}>
          trigger-approve
        </button>
      );
    }

    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1" onOutcome={onOutcome} onError={onError}>
          <ApproveHarness />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(1));
    await act(async () => {
      fireEvent.click(screen.getByText('trigger-approve'));
    });

    // The actionable conflict surfaces its message...
    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith('Branch tasks/00042 already exists outside this fork');
    });
    // ...and the proposal is NOT falsely reported as already resolved.
    expect(onOutcome).not.toHaveBeenCalledWith({ kind: 'already_resolved' });
  });

  // Bug N5 (other branch): a 409 whose refetch shows a TERMINAL status is a
  // genuine resolution in another tab — keep the "already resolved" behavior.
  it('reports already_resolved when an approve 409 refetch shows a terminal status', async () => {
    const onOutcome = vi.fn();
    const onError = vi.fn();
    (api.approveForkProposal as ReturnType<typeof vi.fn>).mockRejectedValue(
      new ConflictError({ error: 'already resolved', error_type: 'proposal_resolved' }),
    );
    const listMock = api.listForkProposals as ReturnType<typeof vi.fn>;
    // Initial: pending. Post-conflict refetch: resolved elsewhere (spawned).
    listMock.mockResolvedValueOnce([proposal({ status: 'pending' })]);
    listMock.mockResolvedValueOnce([
      proposal({ status: 'spawned', fork_conversation_id: 'fork-conv-9' }),
    ]);

    function ApproveHarness() {
      const fork = useForkProposals();
      if (!fork) return null;
      return (
        <button type="button" onClick={() => void fork.approve('prop-1')}>
          trigger-approve
        </button>
      );
    }

    render(
      <MemoryRouter>
        <ForkProposalsProvider conversationId="agent-1" onOutcome={onOutcome} onError={onError}>
          <ApproveHarness />
        </ForkProposalsProvider>
      </MemoryRouter>,
    );

    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(1));
    await act(async () => {
      fireEvent.click(screen.getByText('trigger-approve'));
    });

    await waitFor(() => {
      expect(onOutcome).toHaveBeenCalledWith({ kind: 'already_resolved' });
    });
    // A genuine resolution never surfaces a conflict error.
    expect(onError).not.toHaveBeenCalled();
  });
});
