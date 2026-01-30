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


class PhoenixClient:
    def __init__(self, base_url: str):
        self.base_url = base_url.rstrip('/')
        self.http = httpx.Client(timeout=30.0)

    def get_conversation(self, id_or_slug: str) -> dict:
        """Get conversation by ID or slug."""
        # Try as slug first
        try:
            resp = self.http.get(f"{self.base_url}/api/conversation-by-slug/{id_or_slug}")
            if resp.status_code == 200:
                return resp.json()['conversation']
        except Exception:
            pass

        # Try as ID
        resp = self.http.get(f"{self.base_url}/api/conversation/{id_or_slug}")
        resp.raise_for_status()
        return resp.json()['conversation']

    def create_conversation(self, cwd: str) -> dict:
        """Create new conversation."""
        resp = self.http.post(
            f"{self.base_url}/api/conversations/new",
            json={"cwd": cwd}
        )
        resp.raise_for_status()
        return resp.json()['conversation']

    def send_message(self, conv_id: str, text: str, images: list[dict]) -> None:
        """Send chat message."""
        resp = self.http.post(
            f"{self.base_url}/api/conversation/{conv_id}/chat",
            json={"text": text, "images": images}
        )
        resp.raise_for_status()

    def get_messages(self, conv_id: str, after_sequence: int = 0) -> dict:
        """Get conversation with messages."""
        params = {}
        if after_sequence:
            params["after_sequence"] = after_sequence
        resp = self.http.get(
            f"{self.base_url}/api/conversation/{conv_id}",
            params=params
        )
        resp.raise_for_status()
        return resp.json()

    def stream_until_complete(self, conv_id: str, timeout: float) -> dict:
        """Stream SSE events until conversation is idle or error."""
        url = f"{self.base_url}/api/conversation/{conv_id}/stream"
        messages = []
        conversation = None
        
        with httpx.Client(timeout=httpx.Timeout(timeout)) as client:
            with connect_sse(client, "GET", url) as event_source:
                for event in event_source.iter_sse():
                    if event.event == "init":
                        data = json.loads(event.data)
                        messages = data.get('messages', [])
                        conversation = data.get('conversation')
                        
                    elif event.event == "message":
                        data = json.loads(event.data)
                        msg = data.get('message')
                        if msg:
                            messages.append(msg)
                            
                    elif event.event == "state_change":
                        data = json.loads(event.data)
                        state = data.get('state')
                        if state == 'error':
                            state_data = data.get('state_data', {})
                            error_msg = state_data.get('message', 'Unknown error') if state_data else 'Unknown error'
                            raise PhoenixError(error_msg)
                            
                    elif event.event == "agent_done":
                        # Agent finished, return collected data
                        return {
                            'conversation': conversation,
                            'messages': messages
                        }
                        
                    elif event.event == "error":
                        data = json.loads(event.data)
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
                # Fetch all messages for final output
                return self.get_messages(conv_id)
            elif state == 'error':
                state_data = data['conversation'].get('state_data', {})
                error_msg = state_data.get('message', 'Unknown error') if state_data else 'Unknown error'
                raise PhoenixError(error_msg)

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

    # Determine media type
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

    # Read and encode
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

        lines.append("")  # Blank line between messages

    return "\n".join(lines)


@click.command()
@click.argument('message')
@click.option('-c', '--conversation', envvar='PHOENIX_CONVERSATION',
              help='Conversation ID or slug to continue')
@click.option('-d', '--directory', type=click.Path(exists=True),
              help='Working directory for new conversation')
@click.option('-i', '--image', 'images', multiple=True, type=click.Path(exists=True),
              help='Image file to attach (can be repeated)')
@click.option('--api-url', envvar='PHOENIX_API_URL', default='http://localhost:8000',
              help='API endpoint URL')
@click.option('--timeout', default=600, help='Timeout in seconds')
@click.option('--poll-interval', default=1.0, help='Polling interval in seconds (with --poll)')
@click.option('--poll', is_flag=True, help='Use polling instead of SSE streaming')
def main(message, conversation, directory, images, api_url, timeout, poll_interval, poll):
    """Send a message to Phoenix IDE and wait for response.
    
    Uses SSE (Server-Sent Events) for real-time streaming by default.
    Use --poll for polling fallback mode.
    
    Examples:
    
        # New conversation in current directory
        phoenix-client.py "List the files here"
        
        # Continue existing conversation
        phoenix-client.py -c monday-morning-blue-river "Now create a README"
        
        # With image
        phoenix-client.py -i screenshot.png "What's this error?"
        
        # Use polling instead of SSE
        phoenix-client.py --poll "Hello"
    """
    client = PhoenixClient(api_url)

    # Resolve or create conversation
    if conversation:
        conv = client.get_conversation(conversation)
        click.echo(f"Continuing conversation: {conv.get('slug', conv['id'])}", err=True)
    else:
        cwd = directory or os.getcwd()
        conv = client.create_conversation(cwd)
        click.echo(f"Created conversation: {conv.get('slug', conv['id'])}", err=True)

    # Prepare images
    image_data = [encode_image(path) for path in images]

    # Send message
    click.echo("Sending message...", err=True)
    client.send_message(conv['id'], message, image_data)

    # Wait for completion (SSE or polling)
    if poll:
        click.echo("Waiting for response (polling)...", err=True)
    else:
        click.echo("Streaming response...", err=True)
    
    result = client.wait_for_response(conv['id'], timeout, poll_interval, poll)

    # Format and print output
    print(format_response(result))


def main_with_error_handling():
    try:
        main(standalone_mode=False)
    except PhoenixError as e:
        click.echo(f"Error: {e}", err=True)
        sys.exit(1)
    except httpx.HTTPStatusError as e:
        click.echo(f"API Error: {e.response.status_code} - {e.response.text}", err=True)
        sys.exit(1)
    except KeyboardInterrupt:
        click.echo("\nInterrupted", err=True)
        sys.exit(130)


if __name__ == '__main__':
    main_with_error_handling()
