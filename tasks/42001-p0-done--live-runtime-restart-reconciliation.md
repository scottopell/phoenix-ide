# Live runtime / restart reconciliation

Structurally separate startup/crash recovery interruption synthesis from normal live child-settlement and parent reconciliation. Add deterministic lifecycle-barrier regression coverage proving an active side-effecting tool remains authoritative during live reconciliation and commits its real result exactly once, while genuine startup recovery still synthesizes interruption without a live runtime owner.
