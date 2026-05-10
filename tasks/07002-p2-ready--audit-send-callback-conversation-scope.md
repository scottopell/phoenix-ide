Audit non-SSE async send callbacks for conversation-scope ownership after route switches.

During task 01002, SSE subscription ownership was simplified and guarded. One adjacent non-SSE path remains worth checking separately: `sendMessage` can outlive a route switch, and its catch path calls `dismissRef.current(localId)` / `markFailedRef.current(localId)`. Those refs intentionally point at the current hook instance, so a send started under conversation A may be able to mark/dismiss the current conversation B queue if it fails after navigation.

This is not an SSE / streaming path, so it was not fixed in task 01002. Verify with a focused test and either capture `conversationId`/queue owner in the callback or prove the localId cannot affect a different conversation's queue.
