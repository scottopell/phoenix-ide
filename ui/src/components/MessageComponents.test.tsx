import { describe, expect, it, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { SubAgentStatus, AgentMessage } from './MessageComponents';
import { StreamingMessage } from './StreamingMessage';
import { api, type ConversationState, type Message } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getConversation: vi.fn(),
      getConversationSlug: vi.fn(),
    },
  };
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

function emitInit(source: FakeEventSource, messages: Message[], pendingEvents: unknown[] = []) {
  source.emit('init', {
    type: 'init',
    sequence_id: 100,
    conversation: baseConversation,
    messages,
    agent_working: false,
    last_sequence_id: 100,
    presentation_mode: 'idle',
    context_window_size: 0,
    breadcrumbs: [],
    project_name: null,
    pending_anchor_sequence_id: messages.reduce((max, m) => Math.max(max, m.sequence_id), 0),
    pending_events: pendingEvents,
    pending_truncated: false,
  });
}

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
        <StreamingMessage buffer={{ text: wideTableMarkdown, lastSequence: 1, startedAt: Date.now() }} />
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
        vi.advanceTimersByTime(500);
        await Promise.resolve();
      });
      expect(FakeEventSource.instances).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('caps live child streams to one expanded running sub-agent', async () => {
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

    expect(await screen.findByText(/Another live sub-agent stream/)).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(1);
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
