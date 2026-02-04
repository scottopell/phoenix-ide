#!/bin/bash

# Test Phoenix IDE caching behavior
BASE_URL="http://localhost:7331"

log_timing() {
    local action="$1"
    local duration="$2"
    echo "[$(date +%H:%M:%S.%3N)] $action: ${duration}ms"
}

echo -e "\n=== Phoenix IDE Navigation Performance Test ==="

# 1. Get conversation list
echo -e "\n1. Initial conversation list load:"
start=$(date +%s%3N)
curl -s "$BASE_URL/api/conversations" -o /tmp/conv_list.json
end=$(date +%s%3N)
duration=$((end - start))
log_timing "GET /api/conversations" "$duration"

# Get first conversation slug
SLUG=$(jq -r '.conversations[0].slug' /tmp/conv_list.json)
ID=$(jq -r '.conversations[0].id' /tmp/conv_list.json)
echo "   Selected: $SLUG (id: $ID)"

# 2. Load conversation details
echo -e "\n2. Initial conversation load:"
start=$(date +%s%3N)
curl -s "$BASE_URL/api/conversations/by-slug/$SLUG" -o /tmp/conv_detail.json
end=$(date +%s%3N)
duration=$((end - start))
log_timing "GET /api/conversations/by-slug/$SLUG" "$duration"
MSG_COUNT=$(jq '.messages | length' /tmp/conv_detail.json)
echo "   Messages: $MSG_COUNT"

# 3. Return to list (simulating cache)
echo -e "\n3. Return to conversation list (should be fast on backend):"
start=$(date +%s%3N)
curl -s "$BASE_URL/api/conversations" > /dev/null
end=$(date +%s%3N)
duration=$((end - start))
log_timing "GET /api/conversations" "$duration"

# 4. Reload same conversation
echo -e "\n4. Re-load same conversation (backend has no cache):"
start=$(date +%s%3N)
curl -s "$BASE_URL/api/conversations/by-slug/$SLUG" > /dev/null
end=$(date +%s%3N)
duration=$((end - start))
log_timing "GET /api/conversations/by-slug/$SLUG" "$duration"

# 5. Check response headers
echo -e "\n5. Response headers analysis:"
curl -sI "$BASE_URL/api/conversations/by-slug/$SLUG" | grep -E "(Cache-Control|ETag|Last-Modified|Content-Encoding)"

# 6. Test compression
echo -e "\n6. Test compression effectiveness:"
echo -n "   Without compression: "
curl -s "$BASE_URL/api/conversations" | wc -c
echo -n "   With gzip: "
curl -s -H "Accept-Encoding: gzip" "$BASE_URL/api/conversations" | wc -c
echo -n "   With brotli: "
curl -s -H "Accept-Encoding: br" "$BASE_URL/api/conversations" | wc -c
