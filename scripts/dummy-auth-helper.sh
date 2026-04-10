#!/usr/bin/env bash
# dummy-auth-helper.sh — simulates an interactive OIDC device-flow credential helper.
#
# Usage for testing:
#   LLM_API_KEY_HELPER="bash scripts/dummy-auth-helper.sh" \
#   LLM_API_KEY_HELPER_TTL_MS=10000 \
#   ./dev.py up
#
# The script prints instruction lines (forwarded to the browser), sleeps to simulate
# network round-trips, then exits with the credential on the final line.
# TTL_MS=10000 means the cached credential expires after 10 seconds — useful for
# repeatedly testing the auth flow without restarting the server.
set -euo pipefail

echo "Preparing authentication..."
sleep 0.3
echo ""
echo "Complete the login via your OIDC provider. Open the following link in your browser:"
echo ""
echo "    https://example.com/activate?user_code=ABCD-1234"
echo ""
sleep 0.3
echo "Waiting for OIDC authentication to complete..."
echo "When prompted, enter code: ABCD-1234"
sleep 0.8
# This last line is the credential — held server-side, NOT forwarded to the browser.
echo "dummy-api-key-$(date +%s)"
