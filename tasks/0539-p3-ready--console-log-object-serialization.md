---
created: 2026-02-09
priority: p3
status: ready
---

# Improve Console Log Object Serialization

## Summary

When `console.log({foo: 'bar'})` is called, the captured log shows "Object" instead of the actual object contents.

## Current Behavior

```json
{"level": "log", "text": "Object"}
```

## Desired Behavior

```json
{"level": "log", "text": "{\"foo\": \"bar\"}"}
```

## Root Cause

In `src/tools/browser/session.rs`, we extract console log text from `RemoteObject` fields in this order:
1. `arg.value` - JSON-serializable primitives (strings, numbers, booleans)
2. `arg.description` - String representation (returns "Object" for objects)
3. `arg.unserializable_value` - Special values like undefined, NaN

For objects, `value` is None and `description` is just "Object".

## Solution Options

### Option A: Use CDP to serialize object
Call `Runtime.callFunctionOn` with `JSON.stringify` to get the actual object contents:
```rust
// For each arg that's an object with object_id:
if let Some(obj_id) = &arg.object_id {
    let result = page.execute(
        CallFunctionOnParams::builder()
            .object_id(obj_id.clone())
            .function_declaration("function() { return JSON.stringify(this); }")
            .build()
    ).await?;
    // Use result.value
}
```

### Option B: Use preview field
`RemoteObject` has an `ObjectPreview` that contains property previews. Could reconstruct a simple representation from that.

### Option C: Keep it simple
Just show "Object" for objects - it's what Chrome DevTools does in compact view. Users can `console.log(JSON.stringify(obj))` if they need details.

## Recommendation

Option A is most useful but adds latency. Option C is acceptable for v1. Consider making it configurable.

## Acceptance Criteria

- [ ] Objects logged with console.log show their JSON representation
- [ ] Arrays show their contents
- [ ] Circular references handled gracefully (truncate or show "[Circular]")
- [ ] Large objects truncated to reasonable size
- [ ] Performance acceptable (< 50ms overhead per log entry)

## Files

- `src/tools/browser/session.rs` - `setup_console_listener` event handler
