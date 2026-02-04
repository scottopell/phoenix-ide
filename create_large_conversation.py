#!/usr/bin/env python3
import requests
import json
import time

base_url = "http://localhost:8588"

# Create a new conversation
resp = requests.post(f"{base_url}/api/conversations", json={
    "cwd": "/tmp",
    "model": "claude-3-5-haiku-latest"
})
conv = resp.json()
conv_id = conv["id"]
print(f"Created conversation: {conv_id}")

# Send many messages to create a large conversation
for i in range(60):
    message = f"Test message {i+1} - This is a test message to create a large conversation for testing virtual scrolling. The message needs to be long enough to test variable height rendering."
    
    print(f"Sending message {i+1}/60...")
    resp = requests.post(f"{base_url}/api/conversations/{conv_id}/messages", json={
        "text": message,
        "images": []
    })
    
    if resp.status_code != 200:
        print(f"Error: {resp.status_code} - {resp.text}")
        break
    
    # Give some time for the response
    time.sleep(0.5)

print(f"\nConversation created with many messages!")
print(f"View it at: http://localhost:8588/c/{conv['slug']}")
