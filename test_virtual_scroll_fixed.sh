#!/bin/bash

# Create a new conversation
CONV_JSON=$(curl -s -X POST http://localhost:8588/api/conversations/new \
  -H "Content-Type: application/json" \
  -d '{"cwd": "/tmp"}')

echo "Response: $CONV_JSON"

CONV_ID=$(echo "$CONV_JSON" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
CONV_SLUG=$(echo "$CONV_JSON" | grep -o '"slug":"[^"]*' | cut -d'"' -f4)

if [ -z "$CONV_ID" ]; then
  echo "Failed to create conversation"
  exit 1
fi

echo "Created conversation: $CONV_ID (slug: $CONV_SLUG)"

# Send messages using the correct endpoint
for i in {1..60}; do
  echo -n "Sending message $i/60... "
  
  RESPONSE=$(curl -s -X POST "http://localhost:8588/api/conversations/$CONV_ID/chat" \
    -H "Content-Type: application/json" \
    -d "{\"message\": \"Test message $i - This is a longer test message to create variable heights in the virtual scrolling list. Some messages will be longer than others to test the dynamic height calculation properly.\"}")
  
  if [ $? -eq 0 ]; then
    echo "✓"
  else
    echo "✗"
  fi
  
  sleep 0.1
done

echo ""
echo "✅ Created conversation with 60 messages!"
echo "View at: http://localhost:8588/c/$CONV_SLUG"
