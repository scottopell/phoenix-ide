// Phoenix Mobile-First UI

(function() {
  'use strict';

  // ==========================================================================
  // State
  // ==========================================================================

  const state = {
    view: 'list', // 'list' | 'chat'
    conversations: [],
    currentConversation: null,
    messages: [],
    convState: 'idle',
    stateData: null,
    breadcrumbs: [],
    eventSource: null,
    agentWorking: false,
  };

  // ==========================================================================
  // DOM Elements
  // ==========================================================================

  const $ = (sel) => document.querySelector(sel);
  const $$ = (sel) => document.querySelectorAll(sel);

  const els = {
    stateDot: $('#state-dot'),
    stateText: $('#state-text'),
    convSlug: $('#conv-slug'),
    breadcrumbTrail: $('#breadcrumb-trail'),
    convListView: $('#conversation-list'),
    chatView: $('#chat-view'),
    convList: $('#conv-list'),
    messages: $('#messages'),
    messageInput: $('#message-input'),
    sendBtn: $('#send-btn'),
    newConvBtn: $('#new-conv-btn'),
    modalOverlay: $('#modal-overlay'),
    cwdInput: $('#cwd-input'),
    cwdError: $('#cwd-error'),
    modalCancel: $('#modal-cancel'),
    modalCreate: $('#modal-create'),
  };

  // ==========================================================================
  // API
  // ==========================================================================

  const api = {
    async listConversations() {
      const resp = await fetch('/api/conversations');
      if (!resp.ok) throw new Error('Failed to list conversations');
      return (await resp.json()).conversations;
    },

    async createConversation(cwd) {
      const resp = await fetch('/api/conversations/new', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ cwd }),
      });
      if (!resp.ok) {
        const err = await resp.json();
        throw new Error(err.error || 'Failed to create conversation');
      }
      return (await resp.json()).conversation;
    },

    async sendMessage(convId, text, images = []) {
      const resp = await fetch(`/api/conversations/${convId}/chat`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ text, images }),
      });
      if (!resp.ok) throw new Error('Failed to send message');
      return resp.json();
    },

    async validateCwd(path) {
      const resp = await fetch(`/api/validate-cwd?path=${encodeURIComponent(path)}`);
      return resp.json();
    },

    streamConversation(convId, onEvent) {
      const es = new EventSource(`/api/conversations/${convId}/stream`);
      
      es.addEventListener('init', (e) => {
        const data = JSON.parse(e.data);
        onEvent('init', data);
      });

      es.addEventListener('message', (e) => {
        const data = JSON.parse(e.data);
        onEvent('message', data);
      });

      es.addEventListener('state_change', (e) => {
        const data = JSON.parse(e.data);
        onEvent('state_change', data);
      });

      es.addEventListener('agent_done', (e) => {
        onEvent('agent_done', {});
      });

      es.addEventListener('error', (e) => {
        if (es.readyState === EventSource.CLOSED) {
          onEvent('disconnected', {});
        }
      });

      return es;
    },
  };

  // ==========================================================================
  // Rendering
  // ==========================================================================

  function render() {
    renderStateBar();
    renderBreadcrumbs();
    renderViews();
    renderInputState();
  }

  function renderStateBar() {
    const { convState, stateData, currentConversation, agentWorking } = state;
    
    // State dot
    els.stateDot.className = 'dot';
    if (!state.eventSource || state.eventSource.readyState !== EventSource.OPEN) {
      els.stateDot.classList.add('connecting');
      els.stateText.textContent = 'connecting...';
    } else if (convState === 'idle') {
      els.stateDot.classList.add('idle');
      els.stateText.textContent = 'ready';
    } else if (convState === 'error') {
      els.stateDot.classList.add('error');
      els.stateText.textContent = stateData?.message || 'error';
    } else {
      els.stateDot.classList.add('working');
      els.stateText.textContent = getStateDescription(convState, stateData);
    }

    // Conversation slug
    if (currentConversation) {
      els.convSlug.textContent = currentConversation.slug;
    } else {
      els.convSlug.textContent = '—';
    }
  }

  function getStateDescription(convState, stateData) {
    switch (convState) {
      case 'awaiting_llm':
        return 'preparing request...';
      case 'llm_requesting':
        const attempt = stateData?.attempt || 1;
        return attempt > 1 ? `thinking (retry ${attempt})...` : 'thinking...';
      case 'tool_executing':
        const tool = stateData?.current_tool?.input?._tool || 'tool';
        const remaining = stateData?.remaining_tools?.length || 0;
        return remaining > 0 ? `${tool} (+${remaining} queued)` : tool;
      case 'awaiting_sub_agents':
        const pending = stateData?.pending_ids?.length || 0;
        return `waiting for ${pending} sub-agent${pending !== 1 ? 's' : ''}`;
      case 'cancelling':
      case 'cancelling_llm':
      case 'cancelling_tool':
      case 'cancelling_sub_agents':
        return 'cancelling...';
      default:
        return convState.replace(/_/g, ' ');
    }
  }

  function renderBreadcrumbs() {
    const { breadcrumbs, view } = state;
    
    if (view !== 'chat' || breadcrumbs.length === 0) {
      els.breadcrumbTrail.innerHTML = '';
      return;
    }

    const html = breadcrumbs.map((b, i) => {
      const isLast = i === breadcrumbs.length - 1;
      const activeClass = isLast ? 'active' : '';
      const toolClass = b.type === 'tool' ? 'tool' : '';
      const arrow = i < breadcrumbs.length - 1 ? '<span class="breadcrumb-arrow">→</span>' : '';
      return `<span class="breadcrumb-item ${activeClass} ${toolClass}" data-index="${i}">${b.label}</span>${arrow}`;
    }).join('');

    els.breadcrumbTrail.innerHTML = html;
    
    // Auto-scroll to end
    const bar = $('#breadcrumb-bar');
    bar.scrollLeft = bar.scrollWidth;
  }

  function renderViews() {
    if (state.view === 'list') {
      els.convListView.classList.add('active');
      els.chatView.classList.remove('active');
      renderConversationList();
    } else {
      els.convListView.classList.remove('active');
      els.chatView.classList.add('active');
      renderMessages();
    }
  }

  function renderConversationList() {
    const { conversations } = state;

    if (conversations.length === 0) {
      els.convList.innerHTML = `
        <li class="empty-state">
          <div class="empty-state-icon">💬</div>
          <p>No conversations yet</p>
        </li>
      `;
      return;
    }

    els.convList.innerHTML = conversations.map(conv => `
      <li class="conv-item" data-id="${conv.id}">
        <div class="conv-item-slug">${escapeHtml(conv.slug)}</div>
        <div class="conv-item-meta">
          <span>${formatRelativeTime(conv.updated_at)}</span>
          <span class="conv-item-cwd">${escapeHtml(conv.cwd)}</span>
        </div>
      </li>
    `).join('');
  }

  function renderMessages() {
    const { messages } = state;

    if (messages.length === 0) {
      els.messages.innerHTML = `
        <div class="empty-state">
          <div class="empty-state-icon">✨</div>
          <p>Start a conversation</p>
        </div>
      `;
      return;
    }

    els.messages.innerHTML = messages.map(renderMessage).join('');
    
    // Scroll to bottom
    const main = $('#main-area');
    main.scrollTop = main.scrollHeight;
  }

  function renderMessage(msg) {
    const type = msg.message_type || msg.type;
    
    if (type === 'user') {
      return renderUserMessage(msg);
    } else if (type === 'agent') {
      return renderAgentMessage(msg);
    } else if (type === 'tool') {
      return renderToolResult(msg);
    }
    return '';
  }

  function renderUserMessage(msg) {
    const content = msg.content;
    const text = content.text || (typeof content === 'string' ? content : '');
    const images = content.images || [];
    
    let imageHtml = '';
    if (images.length > 0) {
      imageHtml = `<div style="margin-top: 8px; color: var(--text-muted); font-size: 13px;">[${images.length} image(s)]</div>`;
    }

    return `
      <div class="message user">
        <div class="message-header">You</div>
        <div class="message-content">${escapeHtml(text)}${imageHtml}</div>
      </div>
    `;
  }

  function renderAgentMessage(msg) {
    const content = msg.content;
    const blocks = Array.isArray(content) ? content : [];
    
    let html = '<div class="message agent"><div class="message-header">Phoenix</div><div class="message-content">';

    for (const block of blocks) {
      if (block.type === 'text') {
        html += renderMarkdown(block.text);
      } else if (block.type === 'tool_use') {
        html += renderToolUse(block);
      }
    }

    html += '</div></div>';
    return html;
  }

  function renderToolUse(block) {
    const name = block.name;
    const input = block.input;
    let inputStr;
    
    // Special handling for common tools
    if (name === 'bash' && input.command) {
      inputStr = input.command;
    } else if (name === 'think' && input.thoughts) {
      inputStr = input.thoughts;
    } else {
      inputStr = JSON.stringify(input, null, 2);
    }

    return `
      <div class="tool-block" data-tool-id="${escapeHtml(block.id)}">
        <div class="tool-header" onclick="toggleToolBlock(this)">
          <span class="tool-name">${escapeHtml(name)}</span>
          <span class="tool-chevron">▶</span>
        </div>
        <div class="tool-content">${escapeHtml(inputStr)}</div>
      </div>
    `;
  }

  function renderToolResult(msg) {
    const content = msg.content;
    // Backend uses 'content' field, not 'result'
    const result = content.content || content.result || content.error || '';
    const isError = content.is_error || !!content.error;
    const toolUseId = content.tool_use_id;
    
    // Truncate long results
    const maxLen = 500;
    const truncated = result.length > maxLen;
    const displayResult = truncated ? result.slice(0, maxLen) + '...' : result;

    return `
      <div class="tool-block tool-result ${isError ? 'error' : ''}" data-tool-use-id="${escapeHtml(toolUseId)}">
        <div class="tool-header" onclick="toggleToolBlock(this)">
          <span class="tool-name">result${isError ? ' (error)' : ''}</span>
          ${truncated ? '<span style="color: var(--text-muted); font-size: 11px; margin-left: 8px;">(truncated)</span>' : ''}
          <span class="tool-chevron">▶</span>
        </div>
        <div class="tool-content">${escapeHtml(displayResult)}</div>
      </div>
    `;
  }

  function renderInputState() {
    const { view, agentWorking, currentConversation } = state;
    const canSend = view === 'chat' && !agentWorking && currentConversation;
    
    els.messageInput.disabled = !canSend;
    els.sendBtn.disabled = !canSend;
    
    if (agentWorking) {
      els.sendBtn.innerHTML = '<span class="spinner"></span>';
    } else {
      els.sendBtn.textContent = 'Send';
    }
  }

  // ==========================================================================
  // Markdown (minimal)
  // ==========================================================================

  function renderMarkdown(text) {
    if (!text) return '';
    
    // Escape HTML first
    let html = escapeHtml(text);
    
    // Code blocks
    html = html.replace(/```([\s\S]*?)```/g, '<pre><code>$1</code></pre>');
    
    // Inline code
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
    
    // Bold
    html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    
    // Italic
    html = html.replace(/\*([^*]+)\*/g, '<em>$1</em>');
    
    // Line breaks to paragraphs
    const paragraphs = html.split(/\n\n+/);
    html = paragraphs.map(p => p.trim() ? `<p>${p.replace(/\n/g, '<br>')}</p>` : '').join('');
    
    return html;
  }

  // ==========================================================================
  // Helpers
  // ==========================================================================

  function escapeHtml(str) {
    if (!str) return '';
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function formatRelativeTime(isoStr) {
    if (!isoStr) return '';
    const date = new Date(isoStr);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return date.toLocaleDateString();
  }

  // ==========================================================================
  // Actions
  // ==========================================================================

  async function loadConversations() {
    try {
      state.conversations = await api.listConversations();
      render();
    } catch (err) {
      console.error('Failed to load conversations:', err);
    }
  }

  function openConversation(convId) {
    const conv = state.conversations.find(c => c.id === convId);
    if (!conv) return;

    state.currentConversation = conv;
    state.view = 'chat';
    state.messages = [];
    state.breadcrumbs = [];
    state.convState = 'idle';
    state.agentWorking = false;

    // Close existing SSE
    if (state.eventSource) {
      state.eventSource.close();
    }

    // Connect to SSE
    state.eventSource = api.streamConversation(convId, handleSseEvent);
    render();
  }

  function goToList() {
    if (state.eventSource) {
      state.eventSource.close();
      state.eventSource = null;
    }
    state.view = 'list';
    state.currentConversation = null;
    state.messages = [];
    state.breadcrumbs = [];
    loadConversations();
  }

  function handleSseEvent(eventType, data) {
    switch (eventType) {
      case 'init':
        state.messages = data.messages || [];
        // state can be {type: "idle"} object or just "idle" string
        const convState = data.conversation?.state;
        state.convState = (typeof convState === 'object' ? convState?.type : convState) || 'idle';
        state.stateData = data.conversation?.state_data || null;
        state.agentWorking = data.agent_working || false;
        updateBreadcrumbsFromState();
        break;

      case 'message':
        const msg = data.message;
        if (msg) {
          state.messages.push(msg);
          updateBreadcrumbsFromMessage(msg);
        }
        break;

      case 'state_change':
        state.convState = data.state || 'idle';
        state.stateData = data.state_data || null;
        state.agentWorking = !['idle', 'error', 'completed', 'failed'].includes(state.convState);
        updateBreadcrumbsFromState();
        break;

      case 'agent_done':
        state.agentWorking = false;
        state.convState = 'idle';
        // Clear breadcrumbs for next turn
        state.breadcrumbs = [];
        break;

      case 'disconnected':
        // Try to reconnect after a delay
        setTimeout(() => {
          if (state.currentConversation) {
            openConversation(state.currentConversation.id);
          }
        }, 2000);
        break;
    }
    render();
  }

  function updateBreadcrumbsFromState() {
    const { convState, stateData } = state;
    
    if (convState === 'idle' || convState === 'error') {
      return;
    }

    // Add state-based breadcrumb if not already present
    if (convState === 'awaiting_llm' || convState === 'llm_requesting') {
      if (!state.breadcrumbs.some(b => b.type === 'llm')) {
        state.breadcrumbs.push({ type: 'llm', label: 'LLM' });
      }
    }

    if (convState === 'tool_executing' && stateData?.current_tool) {
      const toolName = stateData.current_tool.input?._tool || 'tool';
      if (!state.breadcrumbs.some(b => b.type === 'tool' && b.label === toolName)) {
        state.breadcrumbs.push({ type: 'tool', label: toolName });
      }
    }
  }

  function updateBreadcrumbsFromMessage(msg) {
    if (msg.type === 'user') {
      // New user message = new turn, reset breadcrumbs
      state.breadcrumbs = [{ type: 'user', label: 'User' }];
    }
  }

  async function sendMessage() {
    const text = els.messageInput.value.trim();
    if (!text || !state.currentConversation || state.agentWorking) return;

    els.messageInput.value = '';
    autoResizeTextarea();
    state.agentWorking = true;
    state.breadcrumbs = [{ type: 'user', label: 'User' }];
    render();

    try {
      await api.sendMessage(state.currentConversation.id, text);
    } catch (err) {
      console.error('Failed to send message:', err);
      state.agentWorking = false;
      render();
    }
  }

  function showNewConvModal() {
    els.modalOverlay.classList.remove('hidden');
    els.cwdInput.focus();
    els.cwdError.classList.add('hidden');
  }

  function hideNewConvModal() {
    els.modalOverlay.classList.add('hidden');
  }

  async function createNewConversation() {
    const cwd = els.cwdInput.value.trim();
    if (!cwd) {
      els.cwdError.textContent = 'Please enter a directory';
      els.cwdError.classList.remove('hidden');
      return;
    }

    // Validate
    const validation = await api.validateCwd(cwd);
    if (!validation.valid) {
      els.cwdError.textContent = validation.error || 'Invalid directory';
      els.cwdError.classList.remove('hidden');
      return;
    }

    try {
      const conv = await api.createConversation(cwd);
      hideNewConvModal();
      state.conversations.unshift(conv);
      openConversation(conv.id);
    } catch (err) {
      els.cwdError.textContent = err.message;
      els.cwdError.classList.remove('hidden');
    }
  }

  function autoResizeTextarea() {
    const ta = els.messageInput;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
  }

  // ==========================================================================
  // Global function for onclick handlers
  // ==========================================================================

  window.toggleToolBlock = function(headerEl) {
    const block = headerEl.parentElement;
    block.classList.toggle('expanded');
  };

  // ==========================================================================
  // Event Listeners
  // ==========================================================================

  function setupEventListeners() {
    // Conversation list clicks
    els.convList.addEventListener('click', (e) => {
      const item = e.target.closest('.conv-item');
      if (item) {
        openConversation(item.dataset.id);
      }
    });

    // Back to list
    els.convSlug.addEventListener('click', () => {
      if (state.view === 'chat') {
        goToList();
      }
    });

    // Send message
    els.sendBtn.addEventListener('click', sendMessage);
    els.messageInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendMessage();
      }
    });
    els.messageInput.addEventListener('input', autoResizeTextarea);

    // New conversation modal
    els.newConvBtn.addEventListener('click', showNewConvModal);
    els.modalCancel.addEventListener('click', hideNewConvModal);
    els.modalCreate.addEventListener('click', createNewConversation);
    els.modalOverlay.addEventListener('click', (e) => {
      if (e.target === els.modalOverlay) {
        hideNewConvModal();
      }
    });
    els.cwdInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        createNewConversation();
      }
    });
  }

  // ==========================================================================
  // Init
  // ==========================================================================

  function init() {
    setupEventListeners();
    loadConversations();
  }

  init();
})();
