#!/bin/bash

# Test script to verify compression is working

set -e

echo "Building and starting Phoenix IDE with compression..."
cd /home/exedev/phoenix-ide

# Build the project
cargo build --release

# Start the server in background
PHOENIX_PORT=8001 ./target/release/phoenix_ide &
SERVER_PID=$!

# Wait for server to start
sleep 3

echo -e "\n=== Testing compression support ==="

# Test 1: Check if server accepts compression
echo -e "\n1. Testing gzip compression:"
curl -s -H "Accept-Encoding: gzip" \
     -H "Accept: application/json" \
     -w "\n   Status: %{http_code}\n   Content-Encoding: %{content_encoding}\n   Size (compressed): %{size_download} bytes\n" \
     -o /tmp/compressed.json \
     http://localhost:8001/api/conversations

# Test 2: Compare with uncompressed
echo -e "\n2. Testing without compression:"
curl -s -H "Accept: application/json" \
     -w "\n   Status: %{http_code}\n   Size (uncompressed): %{size_download} bytes\n" \
     -o /tmp/uncompressed.json \
     http://localhost:8001/api/conversations

# Test 3: Check Brotli support
echo -e "\n3. Testing Brotli compression:"
curl -s -H "Accept-Encoding: br" \
     -H "Accept: application/json" \
     -w "\n   Status: %{http_code}\n   Content-Encoding: %{content_encoding}\n   Size (brotli): %{size_download} bytes\n" \
     -o /tmp/brotli.json \
     http://localhost:8001/api/conversations

# Clean up
kill $SERVER_PID 2>/dev/null || true

echo -e "\n=== Compression test complete ==="
