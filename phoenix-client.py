#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "httpx",
#     "httpx-sse",
#     "click",
# ]
# ///
"""Phoenix IDE simple client for LLM agents.

REQ-CLI-001: Single-Shot Execution
REQ-CLI-002: Conversation Management
REQ-CLI-003: Image Support
REQ-CLI-004: Output Format
REQ-CLI-005: SSE Streaming (--poll for fallback)
REQ-CLI-006: Configuration
REQ-CLI-007: Single File Distribution (uv run)
REQ-CLI-008: Model Selection (--model, --list-models)
REQ-CLI-009: Interaction (--respond, --dismiss-question, --dismiss-error, --cancel-steer, --continue)
REQ-CLI-010: Introspection (--diff, --git-status, --usage, --system-prompt, --tasks, --proposals)
REQ-CLI-011: Discovery (--list-conversations, --search-conversations)
REQ-CLI-012: Platform & Config (--version, --deployment, --env, --mcp-status, --usage-overview, --trajectory-export)
"""

import base64
import json
import os
import sys
import time
from pathlib import Path
from urllib.parse import quote

import click
import httpx
from httpx_sse import connect_sse


class PhoenixError(Exception):
    """Phoenix API error."""
    pass


def _state_kind(state: object) -> str:
    if isinstance(state, dict):
        value = state.get('type')
        return value if isinstance(value, str) else 'unknown'
    return state if isinstance(state, str) else 'unknown'


def _state_error_message(state: object, fallback: str) -> str:
    if not isinstance(state, dict):
        return fallback
    failure = state.get('failure')
    if isinstance(failure, dict) and isinstance(failure.get('message'), str):
        return failure['message']
    message = state.get('message')
    return message if isinstance(message, str) else fallback


def _detect_api_url() -> str:
    """Detect API URL from environment or dev.py conventions.

    Priority: PHOENIX_API_URL env var > dev.py status `URL:` line (carries the
    correct scheme — dev serves https with a self-signed cert) > dev.py port
    (http fallback) > default 8000.
    """
    env_url = os.environ.get('PHOENIX_API_URL')
    if env_url:
        return env_url

    # Try to detect from dev.py status output (worktree-specific port)
    try:
        import subprocess
        result = subprocess.run(
            ['./dev.py', 'status'],
            capture_output=True, text=True, timeout=5,
            cwd=Path(__file__).parent,
        )
        port = None
        for line in result.stdout.splitlines():
            stripped = line.strip()
            # Prefer the authoritative "URL: https://localhost:8034" line —
            # it carries the scheme. dev now serves TLS, so rebuilding
            # "http://localhost:<port>" from the port line sends plaintext to
            # an https socket ("illegal request line").
            if stripped.startswith('URL:'):
                return stripped.split('URL:', 1)[1].strip()
            if 'Phoenix=' in line:
                # Parse "Default ports: Phoenix=8033, Vite=8042"
                for part in line.split(','):
                    if 'Phoenix=' in part:
                        port = part.split('Phoenix=')[1].strip().rstrip(',')
        if port:
            return f"http://localhost:{port}"
    except Exception:
        pass

    return "http://localhost:8000"


# Hosts whose TLS we don't verify by default: dev/prod here serve self-signed
# (or local-CA) certs the script's python env doesn't trust. PHOENIX_TLS_INSECURE
# forces it off for any host; otherwise localhost / 127.0.0.1 / *.local are
# treated as trusted-by-locality. Returns the value to pass as httpx `verify`.
def _tls_verify(url: str) -> bool:
    if os.environ.get('PHOENIX_TLS_INSECURE'):
        return False
    try:
        from urllib.parse import urlparse
        host = (urlparse(url).hostname or '').lower()
    except Exception:
        return True
    if host in ('localhost', '127.0.0.1', '::1') or host.endswith('.local'):
        return False
    return True


class PhoenixClient:
    def __init__(self, base_url: str, password: str | None = None):
        self.base_url = base_url.rstrip('/')
        self.password = password
        # Self-signed dev/prod TLS: don't verify for localhost/*.local (or when
        # PHOENIX_TLS_INSECURE is set). Stored so every client this instance
        # builds (re-auth, SSE) uses the same policy.
        self.verify = _tls_verify(self.base_url)
        self.http = httpx.Client(
            timeout=30.0, headers=self._auth_headers(), verify=self.verify
        )

    def _auth_headers(self) -> dict:
        """Auth headers for API clients. The server authenticates non-browser
        clients via `Authorization: Bearer <password>`; the phoenix-auth cookie
        holds an opaque server-minted session token, not the password, so it is
        not a usable client credential. Empty when no password is configured,
        preserving the auth-disabled/dev case."""
        return {"Authorization": f"Bearer {self.password}"} if self.password else {}

    def check_auth(self) -> dict:
        """Check auth status. Returns { auth_required, authenticated }."""
        resp = self.http.get(f"{self.base_url}/api/auth/status")
        resp.raise_for_status()
        return resp.json()

    def ensure_authenticated(self):
        """Check if auth is required and we're authenticated. Prompt if needed."""
        status = self.check_auth()
        if not status.get('auth_required', False):
            return  # No auth needed
        if status.get('authenticated', False):
            return  # Already authenticated (Bearer password header worked)

        # Auth required but not authenticated
        if self.password:
            # Password was provided but didn't work
            raise PhoenixError(
                "Authentication failed: incorrect password. "
                "Check PHOENIX_PASSWORD or --password value."
            )
        # No password provided -- prompt
        import getpass
        pw = getpass.getpass("Phoenix password: ")
        self.password = pw
        self.http = httpx.Client(
            timeout=30.0, headers=self._auth_headers(), verify=self.verify
        )
        # Verify
        status = self.check_auth()
        if not status.get('authenticated', False):
            raise PhoenixError("Authentication failed: incorrect password.")

    def get_conversation(self, id_or_slug: str) -> dict:
        """Get conversation by ID or slug."""
        # Try as slug first
        try:
            resp = self.http.get(f"{self.base_url}/api/conversations/by-slug/{id_or_slug}")
            if resp.status_code == 200:
                return resp.json()['conversation']
        except Exception:
            pass

        # Try as ID
        resp = self.http.get(f"{self.base_url}/api/conversations/{id_or_slug}")
        resp.raise_for_status()
        return resp.json()['conversation']

    def get_models(self) -> dict:
        """Get available models."""
        resp = self.http.get(f"{self.base_url}/api/models")
        resp.raise_for_status()
        return resp.json()

    def get_projects(self) -> list[dict]:
        """Get all projects."""
        resp = self.http.get(f"{self.base_url}/api/projects")
        resp.raise_for_status()
        return resp.json().get('projects', [])

    def get_wake_status(self, conv_id: str) -> dict:
        """Get active durable wake contracts for a conversation."""
        resp = self.http.get(f"{self.base_url}/api/conversations/{conv_id}/wake")
        resp.raise_for_status()
        return resp.json()

    def cancel_wake(self, conv_id: str, contract_id: str) -> None:
        """Cancel one active durable wake contract."""
        encoded_contract_id = quote(contract_id, safe='')
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/wake/{encoded_contract_id}/cancel"
        )
        resp.raise_for_status()

    # ------------------------------------------------------------------
    # Discovery (REQ-CLI-011)
    # ------------------------------------------------------------------

    def list_conversations(self) -> list[dict]:
        """List all non-archived conversations."""
        resp = self.http.get(f"{self.base_url}/api/conversations")
        resp.raise_for_status()
        return resp.json().get('conversations', [])

    def search_conversations(self, query: str, limit: int | None = None) -> list[dict]:
        """Search conversation contents; returns hits with slug, snippet, score."""
        params: dict = {"q": query}
        if limit is not None:
            params["limit"] = limit
        resp = self.http.get(
            f"{self.base_url}/api/conversations/search", params=params
        )
        resp.raise_for_status()
        return resp.json().get('hits', [])

    # ------------------------------------------------------------------
    # Interaction (REQ-CLI-009)
    # ------------------------------------------------------------------

    def respond_to_question(self, conv_id: str, answers: dict[str, str]) -> dict:
        """Answer a pending user question (AwaitingUserResponse state)."""
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/respond",
            json={"answers": answers},
        )
        resp.raise_for_status()
        return resp.json()

    def dismiss_question(self, conv_id: str) -> dict:
        """Dismiss a pending user question without answering."""
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/dismiss-question"
        )
        resp.raise_for_status()
        return resp.json()

    def dismiss_error(self, conv_id: str) -> dict:
        """Dismiss a user-resumable error, returning the conversation to Idle."""
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/dismiss-error"
        )
        resp.raise_for_status()
        return resp.json()

    def cancel_steering(self, conv_id: str, message_id: str) -> None:
        """Cancel a queued steering message."""
        encoded = quote(message_id, safe='')
        resp = self.http.delete(
            f"{self.base_url}/api/conversations/{conv_id}/steering-queue/{encoded}"
        )
        resp.raise_for_status()

    def continue_conversation(
        self, conv_id: str, handoff: str, message_id: str | None = None
    ) -> dict:
        """Continue a context-exhausted conversation with a handoff message.

        Creates (or returns the existing) successor conversation. The parent
        must be in ContextExhausted state; otherwise the server returns 409.
        Returns {conversation_id, slug?, status, error?} where status is one
        of accepted | dispatch_failed | already_exists.
        """
        import uuid
        payload = {
            "handoff": handoff,
            "message_id": message_id or str(uuid.uuid4()),
        }
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/continue",
            json=payload,
        )
        resp.raise_for_status()
        return resp.json()

    # ------------------------------------------------------------------
    # Introspection (REQ-CLI-010)
    # ------------------------------------------------------------------

    def get_diff(self, conv_id: str) -> dict:
        """Worktree diff against the conversation's base branch."""
        resp = self.http.get(f"{self.base_url}/api/conversations/{conv_id}/diff")
        resp.raise_for_status()
        return resp.json()

    def get_git_status(self, conv_id: str) -> dict:
        """Git status snapshot for the conversation's worktree."""
        resp = self.http.get(
            f"{self.base_url}/api/conversations/{conv_id}/git-status"
        )
        resp.raise_for_status()
        return resp.json()

    def get_usage(self, conv_id: str) -> dict:
        """Token usage totals for a conversation (own + root rollup)."""
        resp = self.http.get(f"{self.base_url}/api/conversations/{conv_id}/usage")
        resp.raise_for_status()
        return resp.json()

    def get_system_prompt(self, conv_id: str) -> dict:
        """Resolved system prompt for the conversation."""
        resp = self.http.get(
            f"{self.base_url}/api/conversations/{conv_id}/system-prompt"
        )
        resp.raise_for_status()
        return resp.json()

    def get_tasks(self, conv_id: str) -> dict:
        """Task files in the conversation's working directory."""
        resp = self.http.get(f"{self.base_url}/api/conversations/{conv_id}/tasks")
        resp.raise_for_status()
        return resp.json()

    def get_proposals(self, conv_id: str) -> dict:
        """Fork proposals for the conversation."""
        resp = self.http.get(
            f"{self.base_url}/api/conversations/{conv_id}/proposals"
        )
        resp.raise_for_status()
        return resp.json()

    # ------------------------------------------------------------------
    # Platform & config (REQ-CLI-012)
    # ------------------------------------------------------------------

    def get_version(self) -> dict:
        """Server build version and git SHA."""
        resp = self.http.get(f"{self.base_url}/api/version")
        resp.raise_for_status()
        return resp.json()

    def get_deployment(self) -> dict:
        """Deployment info: build, network, TLS, resources, disk layout."""
        resp = self.http.get(f"{self.base_url}/api/deployment")
        resp.raise_for_status()
        return resp.json()

    def get_env(self) -> dict:
        """Server environment info (home dir)."""
        resp = self.http.get(f"{self.base_url}/api/env")
        resp.raise_for_status()
        return resp.json()

    def get_mcp_status(self) -> dict:
        """Status of all connected MCP servers."""
        resp = self.http.get(f"{self.base_url}/api/mcp/status")
        resp.raise_for_status()
        return resp.json()

    def get_usage_overview(self) -> dict:
        """Aggregate token usage across all conversations."""
        resp = self.http.get(f"{self.base_url}/api/usage")
        resp.raise_for_status()
        return resp.json()

    def trajectory_export(self, conv_id: str) -> dict:
        """Full trajectory export for a conversation (messages + tool calls)."""
        resp = self.http.get(
            f"{self.base_url}/api/analytics/conversation/{conv_id}/trajectory-export"
        )
        resp.raise_for_status()
        return resp.json()

    def create_conversation(self, cwd: str, text: str, images: list[dict], model: str | None = None) -> dict:
        """Create new conversation with initial message."""
        import uuid
        selected_model = model or self.get_models().get("default")
        if not selected_model:
            raise RuntimeError("server did not advertise a default model")
        payload = {
            "cwd": cwd,
            "model": selected_model,
            "text": text,
            "images": images,
            "message_id": str(uuid.uuid4()),
        }
        resp = self.http.post(
            f"{self.base_url}/api/conversations/new",
            json=payload
        )
        resp.raise_for_status()
        return resp.json()['conversation']

    def send_message(self, conv_id: str, text: str, images: list[dict]) -> None:
        """Send chat message."""
        import uuid
        resp = self.http.post(
            f"{self.base_url}/api/conversations/{conv_id}/chat",
            json={"text": text, "images": images, "message_id": str(uuid.uuid4())}
        )
        resp.raise_for_status()

    def suggest(self, query: str, model: str | None = None) -> list[str]:
        """One-shot shell-command suggestion. Stateless: no conversation.

        /api/suggest is gated by the PHOENIX_SUGGEST_TOKEN capability token
        (injected into Phoenix terminal sessions), not the master password —
        so run this inside a Phoenix terminal, or export the token yourself.
        """
        payload: dict = {"query": query}
        if model:
            payload["model"] = model
        headers = {}
        token = os.environ.get("PHOENIX_SUGGEST_TOKEN")
        if token:
            headers["X-Phoenix-Suggest-Token"] = token
        resp = self.http.post(f"{self.base_url}/api/suggest", json=payload, headers=headers)
        resp.raise_for_status()
        return resp.json().get("commands", [])

    def get_messages(self, conv_id: str, after_sequence: int = 0) -> dict:
        """Get conversation with messages."""
        params = {}
        if after_sequence:
            params["after_sequence"] = after_sequence
        resp = self.http.get(
            f"{self.base_url}/api/conversations/{conv_id}",
            params=params
        )
        resp.raise_for_status()
        return resp.json()

    def stream_until_complete(self, conv_id: str, timeout: float) -> dict:
        """Stream SSE events until conversation is idle or error."""
        url = f"{self.base_url}/api/conversations/{conv_id}/stream"
        messages = []
        conversation = None
        start_time = time.monotonic()

        # SSE read timeout must be generous — the server may not send events
        # for 30+ seconds during tool execution. The overall timeout is enforced
        # by checking elapsed time after each event.
        sse_timeout = httpx.Timeout(
            connect=10.0,
            read=max(timeout, 60.0),  # at least 60s between events
            write=10.0,
            pool=10.0,
        )

        with httpx.Client(
            timeout=sse_timeout, headers=self._auth_headers(), verify=self.verify
        ) as client:
            with connect_sse(client, "GET", url) as event_source:
                for event in event_source.iter_sse():
                    # Check overall timeout
                    elapsed = time.monotonic() - start_time
                    if elapsed > timeout:
                        raise PhoenixError(f"Timeout after {timeout:.0f}s")

                    try:
                        data = json.loads(event.data) if event.data else {}
                    except json.JSONDecodeError:
                        click.echo(f"Warning: malformed SSE event: {event.data[:100]}", err=True)
                        continue

                    if event.event == "init":
                        messages = data.get('messages', [])
                        conversation = data.get('conversation')
                        state = conversation.get('state') if isinstance(conversation, dict) else None
                        state_kind = _state_kind(state)
                        if state_kind == 'recoverable_continuation_failure':
                            raise PhoenixError(
                                _state_error_message(state, 'Continuation summary failed')
                            )
                        if state_kind == 'error':
                            raise PhoenixError(_state_error_message(state, 'Unknown error'))
                        if state_kind == 'context_exhausted':
                            return {'conversation': conversation, 'messages': messages}

                    elif event.event == "message":
                        msg = data.get('message')
                        if msg:
                            messages.append(msg)

                    elif event.event == "state_change":
                        state = data.get('state')
                        state_kind = _state_kind(state)
                        display_state = data.get('display_state')

                        if state_kind == 'error':
                            raise PhoenixError(_state_error_message(state, 'Unknown error'))

                        if state_kind == 'recoverable_continuation_failure':
                            raise PhoenixError(
                                _state_error_message(state, 'Continuation summary failed')
                            )

                        if state_kind == 'context_exhausted':
                            summary = state.get('summary', '') if isinstance(state, dict) else ''
                            click.echo(f"Context exhausted: {summary}", err=True)
                            return {
                                'conversation': conversation,
                                'messages': messages
                            }

                        # Terminal display state also signals completion
                        if display_state == 'terminal':
                            return {
                                'conversation': conversation,
                                'messages': messages
                            }

                    elif event.event == "agent_done":
                        return {
                            'conversation': conversation,
                            'messages': messages
                        }

                    elif event.event == "error":
                        raise PhoenixError(data.get('message', 'Unknown error'))

        # If we exit the loop without agent_done, fetch final state
        return self.get_messages(conv_id)

    def poll_until_complete(self, conv_id: str, timeout: float, interval: float) -> dict:
        """Poll until conversation is idle or error."""
        start = time.time()
        last_sequence = 0

        while time.time() - start < timeout:
            data = self.get_messages(conv_id, last_sequence)
            state = data['conversation']['state']
            state_kind = _state_kind(state)

            if state_kind == 'idle':
                return self.get_messages(conv_id)
            elif state_kind == 'error':
                raise PhoenixError(_state_error_message(state, 'Unknown error'))
            elif state_kind == 'recoverable_continuation_failure':
                raise PhoenixError(
                    _state_error_message(state, 'Continuation summary failed')
                )
            elif state_kind == 'context_exhausted':
                return self.get_messages(conv_id)

            # Update last_sequence for next poll
            if data['messages']:
                last_sequence = max(m['sequence_id'] for m in data['messages'])

            time.sleep(interval)

        raise PhoenixError(f"Timeout after {timeout} seconds")

    def wait_for_response(self, conv_id: str, timeout: float, interval: float, use_polling: bool) -> dict:
        """Wait for response using SSE (default) or polling."""
        if use_polling:
            return self.poll_until_complete(conv_id, timeout, interval)
        else:
            return self.stream_until_complete(conv_id, timeout)


def osc8_run_link(command: str) -> str:
    """Render a command as a clickable OSC 8 hyperlink with a phxrun: URI.

    The visible text is the command itself (prefixed with ▶ to signal it is
    runnable); the link target carries the base64-encoded command. Phoenix's
    terminal intercepts phxrun: links and drops the decoded command onto the
    shell prompt for the user to review and run.
    """
    b64 = base64.b64encode(command.encode()).decode()
    esc, st = "\033", "\033\\"
    return f"{esc}]8;;phxrun:{b64}{st}▶ {command}{esc}]8;;{st}"


def encode_image(path: str) -> dict:
    """Read and base64-encode an image file."""
    p = Path(path)

    suffix = p.suffix.lower()
    media_types = {
        '.png': 'image/png',
        '.jpg': 'image/jpeg',
        '.jpeg': 'image/jpeg',
        '.gif': 'image/gif',
        '.webp': 'image/webp',
    }
    media_type = media_types.get(suffix)
    if not media_type:
        raise click.ClickException(f"Unsupported image format: {suffix}")

    data = p.read_bytes()
    encoded = base64.b64encode(data).decode('ascii')

    return {
        "data": encoded,
        "media_type": media_type
    }


def format_response(data: dict) -> str:
    """Format conversation response for LLM comprehension."""
    lines = []

    for msg in data['messages']:
        msg_type = msg['message_type']
        content = msg['content']

        if msg_type == 'user':
            lines.append("=== USER ===")
            if isinstance(content, dict):
                lines.append(content.get('text', ''))
                if content.get('images'):
                    lines.append(f"[{len(content['images'])} image(s) attached]")
            else:
                lines.append(str(content))

        elif msg_type == 'agent':
            lines.append("=== AGENT ===")
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict):
                        if block.get('type') == 'text':
                            lines.append(block.get('text', ''))
                        elif block.get('type') == 'tool_use':
                            lines.append(f"\n--- TOOL USE: {block.get('name', 'unknown')} ---")
                            lines.append(f"Input: {block.get('input', {})}")
            else:
                lines.append(str(content))

        elif msg_type == 'tool':
            lines.append("--- TOOL RESULT ---")
            if isinstance(content, dict):
                result = content.get('content', content.get('result', str(content)))
                lines.append(str(result))
            else:
                lines.append(str(content))

        elif msg_type == 'system':
            lines.append("=== SYSTEM ===")
            if isinstance(content, dict):
                lines.append(content.get('text', str(content)))
            else:
                lines.append(str(content))

        elif msg_type == 'error':
            lines.append("=== ERROR ===")
            if isinstance(content, dict):
                lines.append(content.get('message', str(content)))
            else:
                lines.append(str(content))

        elif msg_type == 'continuation':
            lines.append("=== CONTEXT EXHAUSTED ===")
            if isinstance(content, dict):
                lines.append(content.get('text', str(content)))
            else:
                lines.append(str(content))

        lines.append("")

    return "\n".join(lines)


def _is_stdout_tty() -> bool:
    return sys.stdout.isatty()


def _print_json(data: object) -> None:
    """Pretty-print JSON for humans (tty) or compact for pipes/agents."""
    if _is_stdout_tty():
        print(json.dumps(data, indent=2, sort_keys=False))
    else:
        print(json.dumps(data, sort_keys=False, separators=(',', ':')))


def _state_str(state: object) -> str:
    """Render a conversation state (string or {type: ...} dict) as a token."""
    if isinstance(state, dict):
        return str(state.get('type') or state.get('kind') or 'unknown')
    return str(state) if state else 'unknown'


def print_conversations_table(conversations: list[dict]) -> None:
    """Compact one-line-per-conversation listing."""
    if not conversations:
        click.echo("No conversations found.")
        return
    for c in conversations:
        slug = c.get('slug') or c['id']
        title = c.get('title') or slug
        state = _state_str(c.get('state'))
        model = c.get('model') or ''
        archived = ' (archived)' if c.get('archived') else ''
        click.echo(f"  {slug:32s} [{state:18s}] {model:20s} {title}{archived}")


def print_search_hits(hits: list[dict]) -> None:
    """Compact search result listing: slug, score, snippet."""
    if not hits:
        click.echo("No matches found.")
        return
    for h in hits:
        slug = h.get('slug') or h.get('conversation_id')
        score = h.get('score', 0.0)
        snippet = (h.get('snippet') or '').replace('\n', ' ')
        if len(snippet) > 120:
            snippet = snippet[:117] + '...'
        archived = ' (archived)' if h.get('archived') else ''
        click.echo(f"  {slug:32s} {score:6.2f}  {snippet}{archived}")


def print_diff(diff: dict) -> None:
    """Delimited worktree diff: committed + uncommitted sections."""
    click.echo(f"=== DIFF (comparator: {diff.get('comparator', '?')}) ===")
    click.echo(f"Label: {diff.get('label', '')}  Kind: {diff.get('kind', '')}")
    if diff.get('pr_number') is not None:
        click.echo(f"PR: #{diff['pr_number']}")
    committed = diff.get('committed_diff') or ''
    if committed:
        click.echo("--- COMMITTED DIFF ---")
        click.echo(committed)
    uncommitted = diff.get('uncommitted_diff') or ''
    if uncommitted:
        click.echo("--- UNCOMMITTED DIFF ---")
        click.echo(uncommitted)
    # Truncation flags so an agent knows the diff was capped.
    if diff.get('committed_truncated_kib') is not None:
        sat = ' (saturated, >=lower bound)' if diff.get('committed_saturated') else ''
        click.echo(
            f"[committed diff truncated: {diff['committed_truncated_kib']} KiB total{sat}]"
        )
    if diff.get('uncommitted_truncated_kib') is not None:
        sat = ' (saturated, >=lower bound)' if diff.get('uncommitted_saturated') else ''
        click.echo(
            f"[uncommitted diff truncated: {diff['uncommitted_truncated_kib']} KiB total{sat}]"
        )


def print_git_status(gs: dict) -> None:
    """Delimited git status snapshot."""
    kind = gs.get('kind', 'snapshot')
    click.echo(f"=== GIT STATUS ({kind}) ===")
    if kind == 'non_git':
        click.echo("Not a git repository.")
        return
    if kind == 'unavailable':
        click.echo(f"Unavailable: {gs.get('reason', 'unknown')}")
        return
    # snapshot
    counts = gs.get('counts') or {}
    click.echo(
        f"Changed: {counts.get('changed_paths', 0)}  "
        f"Staged: {counts.get('staged_paths', 0)}  "
        f"Unstaged: {counts.get('unstaged_paths', 0)}  "
        f"Untracked: {counts.get('untracked_paths', 0)}  "
        f"Conflicted: {counts.get('conflicted_paths', 0)}"
    )
    for p in gs.get('changed_paths') or []:
        click.echo(f"  {p.get('path', '')}  [{p.get('status', '')}]")


def print_proposals(proposals: list[dict]) -> None:
    """Compact fork-proposal listing."""
    if not proposals:
        click.echo("No fork proposals found.")
        return
    for p in proposals:
        click.echo(
            f"  {p.get('id', ''):36s} [{p.get('status', ''):10s}] "
            f"pri={p.get('priority', ''):4s} {p.get('title', '')}"
        )
        if p.get('task_file'):
            click.echo(f"    task: {p['task_file']}")


def print_tasks(tasks: list[dict]) -> None:
    """Compact task-file listing."""
    if not tasks:
        click.echo("No tasks found.")
        return
    for t in tasks:
        click.echo(
            f"  {t.get('id', ''):6s} [{t.get('status', ''):12s}] "
            f"pri={t.get('priority', ''):4s} {t.get('slug', '')}"
        )
        if t.get('conversation_slug'):
            click.echo(f"    owner: {t['conversation_slug']}")


def parse_kv_pairs(pairs: tuple[str, ...]) -> dict[str, str]:
    """Parse repeated --flag KEY=VALUE options into a dict."""
    out: dict[str, str] = {}
    for pair in pairs:
        if '=' not in pair:
            raise click.UsageError(f"Expected KEY=VALUE, got: {pair!r}")
        key, _, value = pair.partition('=')
        key = key.strip()
        if not key:
            raise click.UsageError(f"Empty key in: {pair!r}")
        out[key] = value
    return out


@click.command()
@click.argument('message', required=False)
@click.option('-c', '--conversation', envvar='PHOENIX_CONVERSATION',
              help='Conversation ID or slug to continue')
@click.option('-d', '--directory', type=click.Path(exists=True),
              help='Working directory for new conversation')
@click.option('-i', '--image', 'images', multiple=True, type=click.Path(exists=True),
              help='Image file to attach (can be repeated)')
@click.option('-m', '--model', default=None,
              help='Model ID for new conversations (e.g. claude-4.5-sonnet)')
@click.option('--list-models', is_flag=True, help='List available models and exit')
@click.option('--list-projects', is_flag=True, help='List projects and exit')
@click.option('--wake-status', is_flag=True,
              help='Show active wake contracts for --conversation and exit')
@click.option('--wake-cancel', metavar='CONTRACT_ID',
              help='Cancel an active wake contract for --conversation and exit')
@click.option('--suggest', 'suggest', is_flag=True,
              help='One-shot shell-command suggestion (stateless). MESSAGE may be piped on stdin. '
                   'Emits clickable run-links for the Phoenix terminal.')
# Discovery (REQ-CLI-011)
@click.option('--list-conversations', is_flag=True,
              help='List all conversations and exit')
@click.option('--search-conversations', 'search_conversations', metavar='QUERY', default=None,
              help='Search conversation contents by QUERY and exit')
@click.option('--search-limit', type=int, default=None,
              help='Max hits for --search-conversations (server caps 1-20, default 10)')
# Interaction (REQ-CLI-009) -- require --conversation
@click.option('--respond', 'respond', multiple=True, metavar='KEY=VALUE',
              help='Answer a pending user question (repeatable). Requires --conversation')
@click.option('--dismiss-question', is_flag=True,
              help='Dismiss a pending user question. Requires --conversation')
@click.option('--dismiss-error', is_flag=True,
              help='Dismiss a user-resumable error. Requires --conversation')
@click.option('--cancel-steer', 'cancel_steer', metavar='MSG_ID', default=None,
              help='Cancel a queued steering message by id. Requires --conversation')
@click.option('--continue', 'continue_conv', is_flag=True,
              help='Continue a context-exhausted conversation with a handoff. The handoff text '
                   'comes from MESSAGE or stdin. Requires --conversation')
# Introspection (REQ-CLI-010) -- require --conversation
@click.option('--diff', is_flag=True,
              help='Print the worktree diff for --conversation and exit')
@click.option('--git-status', is_flag=True,
              help='Print the git status for --conversation and exit')
@click.option('--usage', is_flag=True,
              help='Print token usage for --conversation and exit')
@click.option('--system-prompt', is_flag=True,
              help='Print the resolved system prompt for --conversation and exit')
@click.option('--tasks', is_flag=True,
              help='Print task files for --conversation and exit')
@click.option('--proposals', is_flag=True,
              help='Print fork proposals for --conversation and exit')
# Platform & config (REQ-CLI-012)
@click.option('--version', 'show_version', is_flag=True,
              help='Print server version and exit')
@click.option('--deployment', is_flag=True,
              help='Print deployment info and exit')
@click.option('--env', 'show_env', is_flag=True,
              help='Print server environment info and exit')
@click.option('--mcp-status', is_flag=True,
              help='Print MCP server status and exit')
@click.option('--usage-overview', is_flag=True,
              help='Print aggregate token usage across all conversations and exit')
@click.option('--trajectory-export', is_flag=True,
              help='Print full trajectory export for --conversation and exit')
@click.option('--api-url', default=None,
              help='API endpoint URL (default: auto-detect from dev.py or PHOENIX_API_URL)')
@click.option('--timeout', default=600, help='Timeout in seconds')
@click.option('--poll-interval', default=1.0, help='Polling interval in seconds (with --poll)')
@click.option('--poll', is_flag=True, help='Use polling instead of SSE streaming')
@click.option('--password', envvar='PHOENIX_PASSWORD', default=None,
              help='Password for authenticated access (or set PHOENIX_PASSWORD)')
def main(message, conversation, directory, images, model, list_models, list_projects, wake_status, wake_cancel, suggest, list_conversations, search_conversations, search_limit, respond, dismiss_question, dismiss_error, cancel_steer, continue_conv, diff, git_status, usage, system_prompt, tasks, proposals, show_version, deployment, show_env, mcp_status, usage_overview, trajectory_export, api_url, timeout, poll_interval, poll, password):
    """Send a message to Phoenix IDE and wait for response.

    Uses SSE (Server-Sent Events) for real-time streaming by default.
    Use --poll for polling fallback mode.

    Examples:

        # List available models
        phoenix-client.py --list-models

        # List projects
        phoenix-client.py --list-projects

        # New conversation with specific model
        phoenix-client.py -m claude-4.5-sonnet "Analyze this project"

        # New conversation in current directory
        phoenix-client.py "List the files here"

        # Continue existing conversation
        phoenix-client.py -c monday-morning-blue-river "Now create a README"

        # With image
        phoenix-client.py -i screenshot.png "What's this error?"

        # Use polling instead of SSE
        phoenix-client.py --poll "Hello"
    """
    resolved_url = api_url or _detect_api_url()
    client = PhoenixClient(resolved_url, password=password)
    client.ensure_authenticated()

    if list_models:
        data = client.get_models()
        default_model = data.get('default', '')
        for m in data['models']:
            marker = " (default)" if m['id'] == default_model else ""
            click.echo(f"  {m['id']:30s} {m.get('provider', ''):10s} {m.get('description', '')}{marker}")
        return

    if list_projects:
        projects = client.get_projects()
        if not projects:
            click.echo("No projects found.")
        else:
            for p in projects:
                convs = p.get('conversation_count', 0)
                click.echo(f"  {p.get('name', p['id']):30s} {convs} conversation(s)  {p.get('repo_root', '')}")
        return

    if wake_status or wake_cancel:
        if not conversation:
            raise click.UsageError("--wake-status/--wake-cancel requires --conversation.")
        conv = client.get_conversation(conversation)
        if wake_cancel:
            client.cancel_wake(conv['id'], wake_cancel)
            click.echo(f"Cancelled wake contract {wake_cancel}.")
            return
        status = client.get_wake_status(conv['id'])
        click.echo(f"Pending wake contracts: {status['pending_count']}")
        if status.get('soonest_expires_at') is not None:
            click.echo(f"Soonest expiry: {status['soonest_expires_at']}")
        for contract in status.get('contracts', []):
            click.echo(
                f"  {contract['contract_id']}  workflow={contract['workflow_id']}  "
                f"expires_at={contract['expires_at']}"
            )
        return

    if suggest:
        # Query comes from MESSAGE or, if absent, stdin (so prompts can be piped).
        query = message
        if not query and not sys.stdin.isatty():
            query = sys.stdin.read().strip()
        if not query:
            raise click.UsageError("--suggest needs a query as MESSAGE or piped on stdin.")
        commands = client.suggest(query, model=model)
        if not commands:
            click.echo("No commands suggested.", err=True)
            return
        click.echo("Suggested commands (click ▶ to drop onto your prompt):", err=True)
        for command in commands:
            print(osc8_run_link(command))
        return

    # ------------------------------------------------------------------
    # Discovery (REQ-CLI-011)
    # ------------------------------------------------------------------
    if list_conversations:
        print_conversations_table(client.list_conversations())
        return

    if search_conversations is not None:
        hits = client.search_conversations(search_conversations, limit=search_limit)
        print_search_hits(hits)
        return

    # ------------------------------------------------------------------
    # Platform & config (REQ-CLI-012) -- no conversation required
    # ------------------------------------------------------------------
    if show_version:
        _print_json(client.get_version())
        return
    if deployment:
        _print_json(client.get_deployment())
        return
    if show_env:
        _print_json(client.get_env())
        return
    if mcp_status:
        _print_json(client.get_mcp_status())
        return
    if usage_overview:
        _print_json(client.get_usage_overview())
        return

    # ------------------------------------------------------------------
    # Interaction + Introspection (REQ-CLI-009 / 010) -- require -c
    # ------------------------------------------------------------------
    needs_conv = (
        respond
        or dismiss_question
        or dismiss_error
        or cancel_steer
        or continue_conv
        or diff
        or git_status
        or usage
        or system_prompt
        or tasks
        or proposals
        or trajectory_export
    )
    if needs_conv and not conversation:
        raise click.UsageError("this option requires --conversation.")

    if respond or dismiss_question or dismiss_error or cancel_steer:
        conv = client.get_conversation(conversation)
        if respond:
            answers = parse_kv_pairs(respond)
            client.respond_to_question(conv['id'], answers)
            click.echo(f"Responded to question for {conv.get('slug', conv['id'])}.")
            return
        if dismiss_question:
            client.dismiss_question(conv['id'])
            click.echo(f"Dismissed question for {conv.get('slug', conv['id'])}.")
            return
        if dismiss_error:
            client.dismiss_error(conv['id'])
            click.echo(f"Dismissed error for {conv.get('slug', conv['id'])}.")
            return
        if cancel_steer:
            client.cancel_steering(conv['id'], cancel_steer)
            click.echo(f"Cancelled steering message {cancel_steer}.")
            return

    if continue_conv:
        conv = client.get_conversation(conversation)
        handoff = message
        if not handoff and not sys.stdin.isatty():
            handoff = sys.stdin.read().strip()
        if not handoff:
            raise click.UsageError(
                "--continue needs a handoff as MESSAGE or piped on stdin."
            )
        result = client.continue_conversation(conv['id'], handoff)
        slug = result.get('slug') or result.get('conversation_id')
        status = result.get('status', 'unknown')
        click.echo(f"Continuation: {slug}  [{status}]", err=True)
        if result.get('error'):
            click.echo(f"  error: {result['error']}", err=True)
        # Print the new conversation id/slug for the caller to use next.
        print(result.get('conversation_id', ''))
        return

    if diff:
        conv = client.get_conversation(conversation)
        print_diff(client.get_diff(conv['id']))
        return
    if git_status:
        conv = client.get_conversation(conversation)
        print_git_status(client.get_git_status(conv['id']))
        return
    if usage:
        conv = client.get_conversation(conversation)
        _print_json(client.get_usage(conv['id']))
        return
    if system_prompt:
        conv = client.get_conversation(conversation)
        _print_json(client.get_system_prompt(conv['id']))
        return
    if tasks:
        conv = client.get_conversation(conversation)
        print_tasks(client.get_tasks(conv['id']).get('tasks', []))
        return
    if proposals:
        conv = client.get_conversation(conversation)
        print_proposals(client.get_proposals(conv['id']).get('proposals', []))
        return
    if trajectory_export:
        conv = client.get_conversation(conversation)
        _print_json(client.trajectory_export(conv['id']))
        return

    if not message:
        raise click.UsageError(
            "Missing argument 'MESSAGE' (required unless using an info flag "
            "like --list-models, --list-conversations, --version, --diff, ...)."
        )

    # Prepare images
    image_data = [encode_image(path) for path in images]

    if conversation:
        conv = client.get_conversation(conversation)
        mode_label = conv.get('conv_mode_label', '')
        mode_suffix = f" ({mode_label})" if mode_label else ""
        click.echo(f"Continuing conversation: {conv.get('slug', conv['id'])}{mode_suffix}", err=True)
        click.echo("Sending message...", err=True)
        client.send_message(conv['id'], message, image_data)
    else:
        cwd = directory or os.getcwd()
        click.echo("Sending message...", err=True)
        conv = client.create_conversation(cwd, message, image_data, model=model)
        mode_label = conv.get('conv_mode_label', '')
        mode_suffix = f" ({mode_label})" if mode_label else ""
        click.echo(f"Created conversation: {conv.get('slug', conv['id'])}{mode_suffix}", err=True)

    if poll:
        click.echo("Waiting for response (polling)...", err=True)
    else:
        click.echo("Streaming response...", err=True)

    result = client.wait_for_response(conv['id'], timeout, poll_interval, poll)

    print(format_response(result))


def main_with_error_handling():
    try:
        main(standalone_mode=False)
    except PhoenixError as e:
        click.echo(f"Error: {e}", err=True)
        sys.exit(1)
    except httpx.HTTPStatusError as e:
        click.echo(f"API error: {e.response.status_code} - {e.response.text}", err=True)
        sys.exit(1)
    except httpx.ConnectError as e:
        # A TLS verify failure surfaces as ConnectError wrapping an SSLError;
        # don't collapse it into the generic "is it running?" message (which
        # hides the real cause — see task 60409).
        msg = str(e)
        if 'SSL' in msg or 'CERTIFICATE' in msg.upper():
            click.echo(
                f"Error: TLS verification failed connecting to the server: {e}\n"
                "If this is a self-signed dev/prod cert, set PHOENIX_TLS_INSECURE=1 "
                "(localhost/*.local are already trusted by locality).",
                err=True,
            )
        else:
            click.echo(
                f"Error: cannot connect to Phoenix server ({e}). Is it running? (./dev.py up)",
                err=True,
            )
        sys.exit(1)
    except (httpx.ReadTimeout, httpx.ConnectTimeout) as e:
        click.echo(f"Error: connection timed out ({e})", err=True)
        sys.exit(1)
    except httpx.HTTPError as e:
        click.echo(f"HTTP error: {e}", err=True)
        sys.exit(1)
    except KeyboardInterrupt:
        click.echo("\nInterrupted", err=True)
        sys.exit(130)


if __name__ == '__main__':
    main_with_error_handling()
