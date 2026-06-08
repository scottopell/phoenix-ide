# Derive ChainBlock local state props

Task 51001 identified candidate C5 as likely but better handled separately: memoized `ChainBlock` receives global `expandedRowId`, `keyboardSelectedId`, and `activeSlug`, so menu/keyboard/active changes can invalidate every chain block.

Evidence to gather:
- Seed many chains in the sidebar.
- Move keyboard selection and open/close one row menu.
- Count renders for unrelated `ChainBlock` instances and member rows.

Acceptance criteria:
- Derive per-chain booleans/ids before rendering so unrelated chains do not receive changing global props.
- Split chain header/member list if only one subpart needs a global signal.
- Extend existing `ConversationList.test.tsx` memo behavior tests to cover unrelated chains.
