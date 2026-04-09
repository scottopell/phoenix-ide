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
"""

import base64
import json
import os
import sys
import time
from pathlib import Path

import click
import httpx
from httpx_sse import connect_sse


class PhoenixError(Exception):
    """Phoenix API error."""
    pass


def _detect_api_url() -> str:
    """Detect API URL from environment or dev.py conventions.

    Priority: PHOENIX_API_URL env var > dev.py port detection > default 8000.
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
        for line in result.stdout.splitlines():
            if 'Phoenix=' in line:
                # Parse "Default ports: Phoenix=8033, Vite=8042"
                for part in line.split(','):
                    if 'Phoenix=' in part:
                        port = part.split('Phoenix=')[1].strip().rstrip(',')
                        return f"http://localhost:{port}"
    except Exception:
        pass

    return "http://localhost:8000"


class PhoenixClient:
    def __init__(self, base_url: str, password: str | None = None):
        self.base_url = base_url.rstrip('/')
        self.password = password
        cookies = {"phoenix-auth": password} if password else {}
        self.http = httpx.Client(timeout=30.0, cookies=cookies)

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
            return  # Already authenticated (password in cookie worked)

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
            timeout=30.0, cookies={"phoenix-auth": pw}
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

    def create_conversation(self, cwd: str, text: str, images: list[dict], model: str | None = None) -> dict:
        """Create new conversation with initial message."""
        import uuid
        payload = {"cwd": cwd, "text": text, "images": images, "message_id": str(uuid.uuid4())}
        if model:
            payload["model"] = model
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

        cookies = {"phoenix-auth": self.password} if self.password else {}
        with httpx.Client(timeout=sse_timeout, cookies=cookies) as client:
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

                    elif event.event == "message":
                        msg = data.get('message')
                        if msg:
                            messages.append(msg)

                    elif event.event == "state_change":
                        state = data.get('state')
                        display_state = data.get('display_state')

                        if state == 'error':
                            state_data = data.get('state_data', {})
                            error_msg = state_data.get('message', 'Unknown error') if state_data else 'Unknown error'
                            raise PhoenixError(error_msg)

                        if state == 'context_exhausted':
                            state_data = data.get('state_data', {})
                            summary = state_data.get('summary', '') if state_data else ''
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

            # Handle state as either string or dict with type field
            if isinstance(state, dict):
                state = state.get('type', 'unknown')

            if state == 'idle':
                return self.get_messages(conv_id)
            elif state == 'error':
                state_data = data['conversation'].get('state_data', {})
                error_msg = state_data.get('message', 'Unknown error') if state_data else 'Unknown error'
                raise PhoenixError(error_msg)
            elif state == 'context_exhausted':
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
@click.option('--api-url', default=None,
              help='API endpoint URL (default: auto-detect from dev.py or PHOENIX_API_URL)')
@click.option('--timeout', default=600, help='Timeout in seconds')
@click.option('--poll-interval', default=1.0, help='Polling interval in seconds (with --poll)')
@click.option('--poll', is_flag=True, help='Use polling instead of SSE streaming')
@click.option('--password', envvar='PHOENIX_PASSWORD', default=None,
              help='Password for authenticated access (or set PHOENIX_PASSWORD)')
def main(message, conversation, directory, images, model, list_models, list_projects, api_url, timeout, poll_interval, poll, password):
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

    if not message:
        raise click.UsageError("Missing argument 'MESSAGE' (required unless using --list-models).")

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
    except httpx.ConnectError:
        click.echo("Error: cannot connect to Phoenix server. Is it running? (./dev.py up)", err=True)
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
