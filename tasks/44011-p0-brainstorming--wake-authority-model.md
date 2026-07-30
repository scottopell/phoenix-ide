# Wake authority and pure lifecycle model

Replace the superseded PR #559's distributed wake semantics with one authoritative wake-contract aggregate. Define a closed lifecycle, typed cancellation/terminal/forgotten causes, central arbitration policy, canonical terminal evidence with real occurrence time, and a pure total transition function `State + Command -> Outcome + NewState + OwedEffects`.

The repository boundary must atomically commit one terminalization per contract generation: aggregate state/head, terminal receipt, exact delivery, lifecycle event, sequence barrier membership, attempt/lease revocation, and rebuildable projection inputs. Map every historical #559 P1/P2 to an invariant, property, crash-cut, migration, or interleaving test before production integration.

Normal conversation state is not part of this aggregate. Explicit registration may owe later product delivery, but `ToolExecuting -> Idle` remains ordinary conversation settlement.

Supersedes implementation discovery in PR #559; retain branch `task-44009-explicit-bash-wait-until` and its commits as cherry-pick/test evidence.
