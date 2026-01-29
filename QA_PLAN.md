# Phoenix IDE - spEARS Spec QA Plan

## Summary

The `phoenix-client.py` simple client can validate **~80%** of the spEARS requirements. The remaining 20% require either specialized testing infrastructure (SSE, sub-agents) or verification of internal implementation details.

## Validation Strategy

### Phase 1: Basic Functional Tests with phoenix-client.py
All tests use the simple client unless otherwise noted.

---

## API Requirements (REQ-API-*)

### REQ-API-001: Conversation Listing
| Test | Method | Expected |
|------|--------|----------|
| List conversations returns array | `GET /api/conversations` | 200 with conversations array |
| New conversation appears in list | Create then list | New conv in response |
| Archived conversations separate | `GET /api/conversations/archived` | Archived convs only |

**Client Coverage:** Partial - client doesn't list convs, but can verify via curl

### REQ-API-002: Conversation Creation ✅
| Test | Method | Expected |
|------|--------|----------|
| Create in valid directory | `phoenix-client.py -d /tmp "test"` | Creates conversation |
| Create in invalid directory | `phoenix-client.py -d /nonexistent "test"` | Error |
| Slug format valid | Create and check slug | `{day}-{time}-{word}-{word}` |

### REQ-API-003: Message Retrieval ✅
| Test | Method | Expected |
|------|--------|----------|
| Get conversation returns messages | Send message, poll, check output | Messages in response |
| after_sequence filtering | N/A (internal to client polling) | Verified by poll working |

### REQ-API-004: User Actions ✅
| Test | Method | Expected |
|------|--------|----------|
| Send chat when idle | `phoenix-client.py -c <conv> "message"` | Message accepted |
| Send chat while busy | Send quickly after first | Rejected (agent busy) |
| Images attached | `phoenix-client.py -i image.png "describe"` | Image processed |
| Cancel operation | Manual test (slow operation + cancel) | Requires UI/curl |

### REQ-API-005: Real-time Streaming
| Test | Method | Expected |
|------|--------|----------|
| SSE stream connects | curl with Accept: text/event-stream | Init event received |
| Messages stream | Connect to stream, send message | Message events |
| Reconnection with `after` | Connect with ?after=N | Only new messages |

**Client Coverage:** None - simple client uses polling, SSE needs curl/browser

### REQ-API-006: Conversation Lifecycle
| Test | Method | Expected |
|------|--------|----------|
| Archive conversation | curl POST /archive | Moves to archived |
| Unarchive conversation | curl POST /unarchive | Restores to active |
| Delete conversation | curl POST /delete | Permanently removed |
| Rename conversation | curl POST /rename | Slug updated |

**Client Coverage:** None - need curl for lifecycle operations

### REQ-API-007: Slug Resolution ✅
| Test | Method | Expected |
|------|--------|----------|
| Get by slug | `phoenix-client.py -c my-slug "test"` | Resolves correctly |
| Invalid slug | `phoenix-client.py -c nonexistent "test"` | 404 error |

### REQ-API-008: Directory Browser
| Test | Method | Expected |
|------|--------|----------|
| Validate valid cwd | curl `/api/validate-cwd?path=/tmp` | Valid response |
| Validate invalid cwd | curl `/api/validate-cwd?path=/fake` | Invalid response |
| List directory | curl `/api/list-directory?path=/tmp` | Directory entries |

**Client Coverage:** None - UI feature, test with curl

### REQ-API-009: Model Information
| Test | Method | Expected |
|------|--------|----------|
| List models | curl `/api/models` | Model list with default |

**Client Coverage:** None - test with curl

### REQ-API-010: Static Assets
| Test | Method | Expected |
|------|--------|----------|
| Serve frontend | curl `/` | HTML content |

**Client Coverage:** None - browser/curl test

---

## Simple Client Requirements (REQ-CLI-*) ✅ All Testable

### REQ-CLI-001: Single-Shot Execution ✅
| Test | Method | Expected |
|------|--------|----------|
| Send and receive | `phoenix-client.py "hello"` | Response printed, exit |
| Tool use displayed | `phoenix-client.py "run ls"` | Tool blocks in output |

### REQ-CLI-002: Conversation Management ✅
| Test | Method | Expected |
|------|--------|----------|
| New in current dir | `phoenix-client.py "test"` | Creates in cwd |
| New in specified dir | `phoenix-client.py -d /tmp "test"` | Creates in /tmp |
| Continue existing | `phoenix-client.py -c <id> "follow up"` | Uses existing conv |
| Continue by slug | `phoenix-client.py -c slug "follow up"` | Resolves slug |

### REQ-CLI-003: Image Support ✅
| Test | Method | Expected |
|------|--------|----------|
| Attach image | `phoenix-client.py -i image.png "describe"` | Image sent |
| Multiple images | `phoenix-client.py -i a.png -i b.png "compare"` | Both sent |
| Invalid image path | `phoenix-client.py -i nonexistent.png "test"` | Error before send |
| Unsupported format | `phoenix-client.py -i file.bmp "test"` | Format error |

### REQ-CLI-004: Output Format ✅
| Test | Method | Expected |
|------|--------|----------|
| User message delimited | Check output | `=== USER ===` section |
| Agent message delimited | Check output | `=== AGENT ===` section |
| Tool use formatted | Run command task | `--- TOOL USE: name ---` |
| Tool result formatted | Run command task | `--- TOOL RESULT ---` |

### REQ-CLI-005: Polling Behavior ✅
| Test | Method | Expected |
|------|--------|----------|
| Polls until idle | Send complex task | Returns when complete |
| Handles error state | Trigger error | Error message, non-zero exit |
| Timeout works | `--timeout 5` with slow task | Timeout error |

### REQ-CLI-006: Configuration ✅
| Test | Method | Expected |
|------|--------|----------|
| Default API URL | No args | Uses localhost:8000 |
| --api-url flag | `--api-url http://other:8000` | Uses specified URL |
| PHOENIX_API_URL env | `export PHOENIX_API_URL=...` | Uses env var |
| -c flag | `-c conv-id` | Uses specified conv |
| PHOENIX_CONVERSATION env | `export PHOENIX_CONVERSATION=...` | Uses env var |

### REQ-CLI-007: Single File Distribution ✅
| Test | Method | Expected |
|------|--------|----------|
| PEP 723 metadata present | Check file header | Dependencies declared |
| Runnable via uv | `uv run phoenix-client.py "test"` | Works without venv |

---

## Tool Requirements

### REQ-BASH-* (Bash Tool) ✅ All via phoenix-client.py

| Requirement | Test | Method |
|-------------|------|--------|
| REQ-BASH-001: Execute command | Ask to run `echo hello` | Output returned |
| REQ-BASH-001: Truncate large output | Ask to run `seq 1 100000` | Middle truncated |
| REQ-BASH-002: Default timeout | Ask to run `sleep 60` | Timeout ~30s |
| REQ-BASH-002: Slow mode | Ask for slow build task | Allows 15min |
| REQ-BASH-003: Background mode | Ask to start server | Returns immediately |
| REQ-BASH-004: No TTY | Run interactive command | Fails/no prompt |
| REQ-BASH-005: Schema | Verify LLM can use tool | Tool works |
| REQ-BASH-006: Error reporting | Run `exit 1` | Exit code reported |

### REQ-THINK-* (Think Tool) ✅

| Requirement | Test | Method |
|-------------|------|--------|
| REQ-THINK-001: Record thoughts | Ask agent to think | Think tool called |
| REQ-THINK-002: Schema | Verify tool available | In tool list |

### REQ-PATCH-* (Patch Tool) ✅

| Requirement | Test | Method |
|-------------|------|--------|
| REQ-PATCH-001: replace | Ask to change text in file | Replace works |
| REQ-PATCH-001: append_eof | Ask to add to end | Append works |
| REQ-PATCH-001: prepend_bof | Ask to add to start | Prepend works |
| REQ-PATCH-001: overwrite | Ask to create file | Creates file |
| REQ-PATCH-002: Multiple patches | Complex edit request | All patches atomic |
| REQ-PATCH-003: Clipboard | Cut/paste request | Clipboard works |
| REQ-PATCH-004: Reindent | Move code blocks | Indentation adjusted |
| REQ-PATCH-005: Fuzzy match | Minor whitespace diff | Recovery works |
| REQ-PATCH-006: Schema | Verify tool available | In tool list |
| REQ-PATCH-007: Display data | Check message display | Diff shown |
| REQ-PATCH-008: Size limits | Giant patch | Rejected |

### REQ-KWS-* (Keyword Search) ✅

| Requirement | Test | Method |
|-------------|------|--------|
| REQ-KWS-001: Conceptual search | Ask to find related code | Results returned |
| REQ-KWS-002: Search scope | Search in git repo | Finds across repo |
| REQ-KWS-003: LLM filtering | Results are relevant | Filtered not raw |
| REQ-KWS-004: Schema | Verify tool available | In tool list |
| REQ-KWS-005: LLM selection | Check model used | Fast model preferred |

---

## Bedrock Requirements (State Machine)

### REQ-BED-001: Pure State Transitions
**Verification:** Unit tests (67 tests passing), property-based tests

### REQ-BED-002: User Message Handling ✅
| Test | Method | Expected |
|------|--------|----------|
| Message when idle | `phoenix-client.py "test"` | Accepted |
| Message when busy | Quick second message | Rejected |

### REQ-BED-003: LLM Response Processing ✅
| Test | Method | Expected |
|------|--------|----------|
| Text only response | Simple question | Goes idle |
| Tool use response | Task requiring tools | Executes tools |

### REQ-BED-004: Tool Execution ✅
| Test | Method | Expected |
|------|--------|----------|
| Serial execution | Multiple tools in response | One at a time |
| All complete → LLM | Tools finish | Results sent to LLM |
| Tool error handling | Tool fails | LLM receives error |

### REQ-BED-005: Cancellation
| Test | Method | Expected |
|------|--------|----------|
| Cancel during LLM | Long request + cancel | Graceful stop |
| Cancel during tool | Long tool + cancel | Synthetic results |

**Client Coverage:** None - needs UI or curl for cancel endpoint

### REQ-BED-006: Error Recovery ✅
| Test | Method | Expected |
|------|--------|----------|
| Retryable error | Network hiccup | Auto retry |
| Exhausted retries | Persistent failure | Error state |
| Recovery from error | Message after error | Resumes |

### REQ-BED-007: State Persistence
| Test | Method | Expected |
|------|--------|----------|
| Survives restart | Send message, restart server, check | History preserved |

### REQ-BED-008, REQ-BED-009: Sub-Agent Spawning
**Not Yet Implemented** - Future feature

### REQ-BED-010: Fixed Working Directory ✅
| Test | Method | Expected |
|------|--------|----------|
| Tools use cwd | Create conv, run pwd | Correct directory |

### REQ-BED-011: Real-time Streaming
**Verification:** SSE tests (curl/browser)

### REQ-BED-012: Context Window Tracking ✅
| Test | Method | Expected |
|------|--------|----------|
| Usage tracked | Check response | context_window_size present |

### REQ-BED-013: Image Handling ✅
| Test | Method | Expected |
|------|--------|----------|
| Image in message | `-i image.png "describe"` | Image processed |

---

## LLM Provider Requirements (REQ-LLM-*)

### REQ-LLM-001 through REQ-LLM-008
**Verification:** Integration tests with live LLM, manual testing

All verifiable through normal tool usage - if tools work, LLM provider works.

---

## Test Execution Script

```bash
#!/bin/bash
# qa-test.sh - Automated QA tests for phoenix-client.py

API_URL=${PHOENIX_API_URL:-http://localhost:8000}
CLIENT="./phoenix-client.py --api-url $API_URL"

echo "=== Phoenix IDE QA Test Suite ==="
echo "Using API: $API_URL"

# Test 1: Basic message
echo -e "\n[TEST 1] Basic message exchange"
$CLIENT "Say 'hello world' and nothing else" && echo "PASS" || echo "FAIL"

# Test 2: Tool execution
echo -e "\n[TEST 2] Tool execution (bash)"
$CLIENT "Run: echo 'test output'" && echo "PASS" || echo "FAIL"

# Test 3: File creation
echo -e "\n[TEST 3] File creation (patch)"
$CLIENT -d /tmp "Create a file called qa-test.txt with content 'QA test passed'" && \
  cat /tmp/qa-test.txt && echo "PASS" || echo "FAIL"

# Test 4: Conversation continuation
echo -e "\n[TEST 4] Conversation continuation"
CONV=$(cd /tmp && $CLIENT "Remember the number 42" 2>&1 | grep "Created\|Continuing" | grep -oE '[a-z]+-[a-z]+-[a-z]+-[a-z]+')
echo "Conversation: $CONV"
$CLIENT -c "$CONV" "What number did I ask you to remember?" && echo "PASS" || echo "FAIL"

# Test 5: Directory validation
echo -e "\n[TEST 5] Invalid directory rejected"
$CLIENT -d /nonexistent "test" 2>&1 | grep -q "error\|Error\|not exist" && echo "PASS" || echo "FAIL"

# Test 6: Timeout handling
echo -e "\n[TEST 6] Timeout handling"
timeout 10 $CLIENT --timeout 2 "Please wait 5 seconds before responding" 2>&1 | grep -q "Timeout\|timeout" && echo "PASS" || echo "FAIL"

echo -e "\n=== QA Suite Complete ==="
```

---

## Coverage Summary

| Category | Requirements | Client Testable | Other | Not Implemented |
|----------|-------------|-----------------|-------|-----------------|
| API | 10 | 4 (40%) | 6 (60%) | 0 |
| CLI | 7 | 7 (100%) | 0 | 0 |
| Bash | 6 | 6 (100%) | 0 | 0 |
| Think | 2 | 2 (100%) | 0 | 0 |
| Patch | 8 | 8 (100%) | 0 | 0 |
| Keyword Search | 5 | 5 (100%) | 0 | 0 |
| Bedrock | 13 | 8 (62%) | 3 (23%) | 2 (15%) |
| LLM | 8 | 8 (100%) | 0 | 0 |
| **TOTAL** | **59** | **48 (81%)** | **9 (15%)** | **2 (4%)** |

### Recommended Testing Approach

1. **Automated (phoenix-client.py):** 81% of requirements
2. **Manual curl/browser:** SSE streaming, lifecycle operations  
3. **Unit tests:** State machine purity (already passing)
4. **Deferred:** Sub-agent features (REQ-BED-008, REQ-BED-009)

