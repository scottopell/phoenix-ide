# Eliminate asynchronous React act warnings in workflow tests

`NewConversationPage.workflow.test.tsx` passes but repeatedly reports state updates outside `act(...)`, especially around persisted-draft and post-create flows. These warnings mean some asynchronous component work outlives the test action/witness and can hide assertion ordering defects.

Identify each unresolved async producer, drive it through Testing Library/user-event or explicit deferred-promise resolution, and wait for the user-visible settled state. Do not silence `console.error`, restore real-time sleeps, or blanket-wrap unrelated code in `act`. Add a test-lane guard that makes unexpected React act warnings fail this suite.
