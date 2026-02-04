#!/bin/bash

# Create a new conversation
CONV_JSON=$(curl -s -X POST http://localhost:8588/api/conversations \
  -H "Content-Type: application/json" \
  -d '{"cwd": "/tmp", "model": "claude-3-5-haiku-latest"}')

CONV_ID=$(echo "$CONV_JSON" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
CONV_SLUG=$(echo "$CONV_JSON" | grep -o '"slug":"[^"]*' | cut -d'"' -f4)

echo "Created conversation: $CONV_ID (slug: $CONV_SLUG)"

# Send 60 messages
for i in {1..60}; do
  echo "Sending message $i/60..."
  curl -s -X POST "http://localhost:8588/api/conversations/$CONV_ID/messages" \
    -H "Content-Type: application/json" \
    -d "{\"text\": \"Test message $i - This is a longer test message to create variable heights in the virtual scrolling list. Some messages will be longer than others to test the dynamic height calculation properly.\", \"images\": []}" > /dev/null
  
  # Small delay to not overwhelm the server
  sleep 0.2
done

echo ""
echo "✅ Created conversation with 60 messages!"
echo "View at: http://localhost:8588/c/$CONV_SLUG"
