---
created: 2026-04-11
priority: p1
status: done
artifact: pending
---

# fix-react-perf-antipatterns-conversation-view

## Plan

# Fix React Performance Antipatterns — Conversation View & Input Sluggishness

## Summary

A code audit identified 8 concrete React antipatterns causing unnecessary re-renders and expensive work in the critical typing path. The root cascade: every keystroke → `setDraft` → `InputArea` re-renders → `fuzzyMatch` runs unmemoized (O(n log n)) → `handleKeyDown` recreated (depends on `filteredItems`) → inline `onChange`/`onSelect` recreated → downstream `MessageList` gets unstable prop references → `toolResults` Map rebuilt → child components re-render needlessly.

No new features. No behaviour changes. Pure optimization.

## Files Touched

- `ui/src/components/InputArea.tsx`
- `ui/src/components/MessageComponents.tsx`
- `ui/src/components/MessageList.tsx`
- `ui/src/components/StreamingMessage.tsx`
- `ui/src/components/Sidebar.tsx`

---

## Changes (in priority order)

### 1. `InputArea.tsx` — Memoize `filteredItems` (🔴 Highest impact)

**Line 322** — `fuzzyMatch` is called bare in the render body, running on every render:

```ts
// Before
const filteredItems = fuzzyMatch(acItems, activeTrigger?.query ?? '', (item) => item.label);

// After
const filteredItems = useMemo(
  () => fuzzyMatch(acItems, activeTrigger?.query ?? '', (item) => item.label),
  [acItems, activeTrigger?.query],
);
```

This is the highest-impact single fix. `filteredItems` being stable also stabilizes `handleKeyDown`'s dependency array (line 373), which currently recreates the handler every keystroke.

---

### 2. `InputArea.tsx` — Extract inline `onChange` / `onSelect` to `useCallback`

**Lines 649–669** — Both handlers are arrow functions in JSX (new reference every render):

```ts
// Before (in JSX)
onChange={(e) => { ... handleTextChange(newVal); }}
onSelect={() => { ... setActiveTrigger(trigger); }}

// After
const handleChange = useCallback((e: React.ChangeEvent<HTMLTextAreaElement>) => {
  const newVal = e.target.value;
  if (voiceBase !== null) {
    setVoiceBase(newVal);
    setVoiceInterim('');
  } else {
    setDraft(newVal);
  }
  handleTextChange(newVal);
}, [voiceBase, setVoiceBase, setVoiceInterim, setDraft, handleTextChange]);

const handleSelect = useCallback(() => {
  const ta = textareaRef.current;
  if (ta) {
    const currentVal = voiceBase !== null ? voiceBase : draft;
    const trigger = detectTrigger(currentVal, ta.selectionStart);
    setActiveTrigger(trigger);
  }
}, [voiceBase, draft]);

// In JSX
onChange={handleChange}
onSelect={handleSelect}
```

---

### 3. `InputArea.tsx` — Wrap `autoResize` in `useCallback`

**Line 380** — Plain function defined inside render, recreated every time:

```ts
// Before
const autoResize = () => {
  const ta = textareaRef.current;
  if (ta) { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 120) + 'px'; }
};
useEffect(() => { autoResize(); }, [draft]);

// After
const autoResize = useCallback(() => {
  const ta = textareaRef.current;
  if (ta) { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, 120) + 'px'; }
}, []);
useEffect(() => { autoResize(); }, [draft, autoResize]);
```

---

### 4. `MessageComponents.tsx` — Extract `ReactMarkdown` `components` prop out of render

**Lines 245–308** — A fresh `{ code, p, li }` object is created inside `blocks.map()` on every render. ReactMarkdown sees new component definitions every time → remounts syntax highlighters → flickers during streaming.

The only external dependency is `onOpenFile`. Extract the components object into a `useMemo` at the top of `AgentMessage`, keyed on `onOpenFile`:

```ts
// Before (inside blocks.map())
<ReactMarkdown remarkPlugins={[remarkGfm]} components={{ code: ..., p: ..., li: ... }}>

// After — at top of AgentMessage, before the return
const markdownComponents = useMemo(() => ({
  code: ({ inline, className, children, ...props }: { ... }) => {
    // full existing code block implementation
  },
  p: ({ children }: { children?: React.ReactNode }) => {
    const fileClickHandler = onOpenFile
      ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
      : undefined;
    // full existing p implementation
  },
  li: ({ children }: { children?: React.ReactNode }) => {
    const fileClickHandler = onOpenFile
      ? (filePath: string) => onOpenFile(filePath, new Set(), 0)
      : undefined;
    // full existing li implementation
  },
}), [onOpenFile]);

// In JSX
<ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
```

Note: `remarkPlugins={[remarkGfm]}` also creates a new array each render — hoist it to a module-level constant:
```ts
const REMARK_PLUGINS = [remarkGfm];
// then: remarkPlugins={REMARK_PLUGINS}
```
Apply this same hoist everywhere `ReactMarkdown` is used in this file and in `StreamingMessage.tsx`.

---

### 5. `MessageList.tsx` — Memoize `toolResults` Map and `sendingMessages` array

**Lines 154–167** — Both computed values are recreated on every render:

```ts
// Before
const toolResults = new Map<string, Message>();
for (const msg of messages) { ... }
const sendingMessages = queuedMessages.filter(m => m.status === 'sending');

// After
const toolResults = useMemo(() => {
  const map = new Map<string, Message>();
  for (const msg of messages) {
    const type = msg.message_type || msg.type;
    if (type === 'tool') {
      const content = msg.content as ToolResultContent;
      const toolUseId = content?.tool_use_id;
      if (toolUseId) map.set(toolUseId, msg);
    }
  }
  return map;
}, [messages]);

const sendingMessages = useMemo(
  () => queuedMessages.filter(m => m.status === 'sending'),
  [queuedMessages],
);
```

---

### 6. `MessageList.tsx` — Move inline `new RegExp` out of `messages.map()`

**Line 203** — Regex compiled per message per render:

```ts
// Before (inside messages.map())
const triggerArgs = skillTrigger.replace(new RegExp(`^/?${skillContent.name || ''}\\s*`), '').trim();

// After — extract to a helper
function extractSkillArgs(trigger: string, name: string): string {
  return trigger.replace(new RegExp(`^/?${name}\\s*`), '').trim();
}
// then:
const triggerArgs = extractSkillArgs(skillTrigger, skillContent.name || '');
```

---

### 7. `StreamingMessage.tsx` — Fix `key={i}` index key

**Line 63** — Array index as key is unsafe when blocks can shift:

```ts
// Before
{blocks.map((block, i) => (
  <StreamingBlock key={i} block={block} />
))}

// After
{blocks.map((block, i) => (
  <StreamingBlock key={`${block.type}-${i}`} block={block} />
))}
```

This is a minimal safe fix. It doesn't fully solve the stability problem (blocks could theoretically shift) but it at least avoids key collisions across different block types and matches React best-practice guidance for the streaming-append-only pattern.

---

### 8. `Sidebar.tsx` — Extract inline `onDelete`/`onRename` callbacks

**Lines 184–185** — Two of the four `ConversationList` callbacks are inline while the others are `useCallback`:

```ts
// Before
onDelete={(conv) => setDeleteTarget(conv)}
onRename={(conv) => { setRenameError(undefined); setRenameTarget(conv); }}

// After — alongside the existing handleArchive/handleUnarchive useCallbacks
const handleDelete = useCallback((conv: Conversation) => {
  setDeleteTarget(conv);
}, []);

const handleRename = useCallback((conv: Conversation) => {
  setRenameError(undefined);
  setRenameTarget(conv);
}, []);

// In JSX
onDelete={handleDelete}
onRename={handleRename}
```

---

## Acceptance Criteria

- [ ] `filteredItems` in `InputArea` is wrapped in `useMemo` with `[acItems, activeTrigger?.query]` deps
- [ ] `onChange` and `onSelect` on the `<textarea>` in `InputArea` use stable `useCallback` references
- [ ] `autoResize` in `InputArea` is wrapped in `useCallback`
- [ ] `AgentMessage`'s `components` object for `ReactMarkdown` is created via `useMemo([onOpenFile])`, not inline in `blocks.map()`
- [ ] `[remarkGfm]` plugin arrays are hoisted to module-level constants in `MessageComponents.tsx` and `StreamingMessage.tsx`
- [ ] `toolResults` Map in `MessageList` is wrapped in `useMemo([messages])`
- [ ] `sendingMessages` in `MessageList` is wrapped in `useMemo([queuedMessages])`
- [ ] Inline `new RegExp` in `MessageList`'s `messages.map()` is moved to a helper function
- [ ] `key={i}` in `StreamingMessage`'s `blocks.map()` is changed to `key={block.type + '-' + i}`
- [ ] `onDelete` and `onRename` callbacks in `Sidebar` are extracted to `useCallback`
- [ ] No visual regressions — conversation view renders identically before and after
- [ ] `./dev.py check` passes (clippy + fmt + tests)
- [ ] Task file created as `tasks/08660-p1-in-progress--fix-react-perf-antipatterns-conversation-view.md`


## Progress

