#!/usr/bin/env python3

import time
import requests
import json
from datetime import datetime

# Test Phoenix IDE caching behavior
base_url = "http://localhost:7331"

def log_timing(action, start_time):
    duration = (time.time() - start_time) * 1000
    print(f"[{datetime.now().strftime('%H:%M:%S.%f')[:-3]}] {action}: {duration:.1f}ms")

def test_navigation():
    print("\n=== Phoenix IDE Navigation Performance Test ===")
    
    # 1. Get conversation list
    print("\n1. Initial conversation list load:")
    start = time.time()
    resp = requests.get(f"{base_url}/api/conversations")
    log_timing("GET /api/conversations", start)
    
    conversations = resp.json()['conversations']
    if not conversations:
        print("No conversations found!")
        return
    
    # Pick first conversation
    conv = conversations[0]
    slug = conv['slug']
    conv_id = conv['id']
    print(f"   Selected: {slug} (id: {conv_id})")
    
    # 2. Load conversation details
    print("\n2. Initial conversation load:")
    start = time.time()
    resp = requests.get(f"{base_url}/api/conversations/by-slug/{slug}")
    log_timing(f"GET /api/conversations/by-slug/{slug}", start)
    data = resp.json()
    print(f"   Messages: {len(data['messages'])}, Working: {data['agent_working']}")
    
    # 3. Simulate going back to list (would be cached in frontend)
    print("\n3. Return to conversation list (should be cached):")
    start = time.time()
    resp = requests.get(f"{base_url}/api/conversations")
    log_timing("GET /api/conversations", start)
    
    # 4. Load same conversation again
    print("\n4. Re-load same conversation (should be cached):")
    start = time.time()
    resp = requests.get(f"{base_url}/api/conversations/by-slug/{slug}")
    log_timing(f"GET /api/conversations/by-slug/{slug}", start)
    
    # 5. Check with If-None-Match (if supported)
    print("\n5. Check ETag support:")
    etag = resp.headers.get('ETag', 'Not supported')
    print(f"   ETag: {etag}")
    
    # 6. Test compression
    print("\n6. Test compression:")
    headers = {'Accept-Encoding': 'gzip'}
    start = time.time()
    resp = requests.get(f"{base_url}/api/conversations", headers=headers)
    log_timing("GET /api/conversations (gzip)", start)
    print(f"   Content-Encoding: {resp.headers.get('Content-Encoding', 'none')}")
    print(f"   Response size: {len(resp.content)} bytes")
    
    # 7. Analyze response headers
    print("\n7. Response headers analysis:")
    resp = requests.get(f"{base_url}/api/conversations/by-slug/{slug}")
    for header in ['Cache-Control', 'ETag', 'Last-Modified', 'Vary']:
        value = resp.headers.get(header, 'Not set')
        print(f"   {header}: {value}")

if __name__ == "__main__":
    test_navigation()
