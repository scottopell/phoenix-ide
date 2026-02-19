# Simple Client - Design Document

## Overview

A single-file Python CLI client for interacting with the Phoenix API. Designed for LLM agents to use as a tool, with clear structured output and single-shot execution.

## File Structure (REQ-CLI-007)

```python
#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "httpx",
#     "click",
# ]
# ///
"""Phoenix IDE simple client for LLM agents."""

import click
import httpx
import os
import sys
import time
import base64
from pathlib import Path
```

Run with: `uv run phoenix-client.py "your message here"`

## CLI Interface (REQ-CLI-001, REQ-CLI-002, REQ-CLI-003, REQ-CLI-006)

```
Usage: phoenix-client.py [OPTIONS] MESSAGE

Options:
  -c, --conversation TEXT   Conversation ID or slug to continue
  -d, --directory PATH      Working directory for new conversation
  -i, --image PATH          Image file to attach (can be repeated)
  -m, --model TEXT          Model ID for new conversations (e.g. claude-4.5-sonnet)
  --list-models             List available models and exit
  --api-url TEXT            API endpoint URL
  --timeout INTEGER         Polling timeout in seconds (default: 600)
  --poll-interval FLOAT     Polling interval in seconds (default: 1.0)
  --help                    Show this message and exit.
```

### Examples

```bash
# New conversation in current directory
uv run phoenix-client.py "List the files in this directory"

# Continue existing conversation
uv run phoenix-client.py -c monday-morning-blue-river "Now create a README"

# New conversation in specific directory
uv run phoenix-client.py -d /home/user/project "Analyze this codebase"

# With image attachment
uv run phoenix-client.py -i screenshot.png "What's wrong with this error?"

# Multiple images
uv run phoenix-client.py -i before.png -i after.png "Compare these screenshots"
```

### Model Selection Examples (REQ-CLI-008)

```bash
# List available models
uv run phoenix-client.py --list-models --api-url http://localhost:8033

# New conversation with specific model
uv run phoenix-client.py -m claude-4.5-sonnet "Analyze this codebase"
```

## Model Listing (REQ-CLI-008)

```python
def list_models(client: PhoenixClient) -> None:
    """Fetch and display available models."""
    data = client.get_models()
    for m in data['models']:
        default = " (default)" if m['id'] == data.get('default') else ""
        click.echo(f"  {m['id']:30s} {m.get('provider', ''):10s} {m.get('description', '')}{default}")
```

## Main Flow

```python
@click.command()
@click.argument('message')
@click.option('-c', '--conversation', envvar='PHOENIX_CONVERSATION')
@click.option('-d', '--directory', type=click.Path(exists=True))
@click.option('-i', '--image', 'images', multiple=True, type=click.Path(exists=True))
@click.option('--api-url', envvar='PHOENIX_API_URL', default='http://localhost:8000')
@click.option('--timeout', default=600, help='Polling timeout in seconds')
@click.option('--poll-interval', default=1.0, help='Polling interval in seconds')
def main(message, conversation, directory, images, api_url, timeout, poll_interval):
    client = PhoenixClient(api_url)
    
    # Resolve or create conversation
    if conversation:
        conv = client.get_conversation(conversation)
    else:
        cwd = directory or os.getcwd()
        conv = client.create_conversation(cwd)
    
    # Prepare images
    image_data = [encode_image(path) for path in images]
    
    # Send message
    client.send_message(conv['id'], message, image_data)
    
    # Poll for completion
    result = client.poll_until_complete(conv['id'], timeout, poll_interval)
    
    # Format and print output
    print(format_response(result))
```

## API Client

```python
class PhoenixClient:
    def __init__(self, base_url: str):
        self.base_url = base_url.rstrip('/')
        self.http = httpx.Client(timeout=30.0)
    
    def get_conversation(self, id_or_slug: str) -> dict:
        """Get conversation by ID or slug."""
        # Try as slug first
        resp = self.http.get(f"{self.base_url}/api/conversation-by-slug/{id_or_slug}")
        if resp.status_code == 200:
            return resp.json()['conversation']
        
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
        resp = self.http.get(
            f"{self.base_url}/api/conversation/{conv_id}",
            params={"after_sequence": after_sequence} if after_sequence else {}
        )
        resp.raise_for_status()
        return resp.json()
    
    def poll_until_complete(self, conv_id: str, timeout: float, interval: float) -> dict:
        """Poll until conversation is idle or error."""
        start = time.time()
        last_sequence = 0
        
        while time.time() - start < timeout:
            data = self.get_messages(conv_id, last_sequence)
            state = data['conversation']['state']
            
            if state == 'idle':
                return data
            elif state == 'error':
                raise PhoenixError(data['conversation'].get('state_data', {}).get('message', 'Unknown error'))
            
            # Update last_sequence for next poll
            if data['messages']:
                last_sequence = max(m['sequence_id'] for m in data['messages'])
            
            time.sleep(interval)
        
        raise PhoenixError(f"Timeout after {timeout} seconds")
```

## Image Encoding (REQ-CLI-003)

```python
def encode_image(path: str) -> dict:
    """Read and base64-encode an image file."""
    path = Path(path)
    
    # Determine media type
    suffix = path.suffix.lower()
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
    data = path.read_bytes()
    encoded = base64.b64encode(data).decode('ascii')
    
    return {
        "data": encoded,
        "media_type": media_type
    }
```

## Output Formatting (REQ-CLI-004)

```python
def format_response(data: dict) -> str:
    """Format conversation response for LLM comprehension."""
    lines = []
    
    for msg in data['messages']:
        msg_type = msg['type']
        content = msg['content']
        
        if msg_type == 'user':
            lines.append(f"=== USER ===")
            lines.append(content.get('text', ''))
            if content.get('images'):
                lines.append(f"[{len(content['images'])} image(s) attached]")
        
        elif msg_type == 'agent':
            lines.append(f"=== AGENT ===")
            for block in content:
                if block.get('type') == 'text':
                    lines.append(block['text'])
                elif block.get('type') == 'tool_use':
                    lines.append(f"\n--- TOOL USE: {block['name']} ---")
                    lines.append(f"Input: {block['input']}")
        
        elif msg_type == 'tool':
            lines.append(f"--- TOOL RESULT ---")
            lines.append(content.get('result', content.get('error', '')))
        
        lines.append("")  # Blank line between messages
    
    return "\n".join(lines)
```

## Error Handling

```python
class PhoenixError(Exception):
    """Phoenix API error."""
    pass

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
```

## Testing Strategy

### Manual Testing
- New conversation creation
- Continuing existing conversation by slug
- Continuing existing conversation by ID
- Image attachment (single and multiple)
- Timeout behavior
- Error state handling

### Integration Testing
Requires running Phoenix server:
```bash
# Start server
phoenix serve &

# Test basic flow
uv run phoenix-client.py "echo hello"

# Test conversation continuation
CONV=$(uv run phoenix-client.py "what is 2+2" | grep -o 'conversation: [a-z-]*' | cut -d' ' -f2)
uv run phoenix-client.py -c $CONV "multiply that by 3"
```
