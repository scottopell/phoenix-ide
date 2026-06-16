ToolOutput -- the type EVERY Tool::run() returns -- is still the contradictory `success: bool` + `output: String` shape that ToolOutcome was explicitly created to eliminate. The producer-side type regresses the exact named pattern the persisted type fixed.

Verified locations:
- crates/phoenix-ide/src/tools.rs:67-74 -- `pub struct ToolOutput { pub success: bool, pub output: String, ... }`
- crates/phoenix-ide/src/db/schema.rs:524-547 -- ToolOutcome enum. Its own doc comment: "Outcome of a tool execution. Replaces the contradictory `success: bool` + `is_error: bool` pair -- this enum makes the three meaningful states explicit and the fourth (`success=false`, `is_error=false` but not cancelled) unrepresentable."
- crates/phoenix-ide/src/runtime/executor.rs:1917-1929 -- lossy mapping `if out.success { Success } else { Error }`; ToolOutput cannot represent Cancelled at all (synthesized elsewhere).

Why egregious: the codebase wrote down, in the sibling type's doc comment, exactly why bool is wrong and made the bad state unrepresentable in the persisted enum -- then left the source type every tool returns as bool+String. A tool returning success=true with error-shaped output is structurally indistinguishable from a real success. There is documented harm from reasoning about success via stringly/booly tool output: 08545-p1-done--spurious-tool-aborted-from-output-string was a P1 conversation-stuck incident rooted in inferring outcome state from out.output string content.

Related tasks:
- 08545-p1-done (symptom: executor inferred cancellation from out.output string; fixed the executor inference, not the type).
- 08007-p3-done--stronger-typing-for-message-content (adjacent typing effort; did not cover ToolOutput).

Fix direction: replace ToolOutput's success:bool+output:String with an outcome enum mirroring ToolOutcome (Success/Error/Cancelled carrying typed payloads), so Tool::run() cannot construct a contradictory result and the executor mapping at executor.rs:1917-1929 becomes total/structural rather than an if/else on a bool. Audit all Tool impls and ToolOutput::success/error constructors.
