# Support drag-and-drop file attachments in chat

## Context

Phoenix already supports image attachments in the chat composer and new-conversation composer:

- `ui/src/components/InputArea.tsx` accepts pasted image files, stores them in `images`, previews them with `ImageAttachments`, sends them through `api.sendMessage`, and restores them on expansion errors.
- `ui/src/hooks/useCreateConversation.ts` and `ui/src/pages/NewConversationPage.tsx` have the analogous new-conversation flow.
- `crates/phoenix-ide/src/api/types.rs` accepts `images: Vec<ImageAttachment>` on both chat and create-conversation requests.
- `crates/phoenix-core/src/domain/db_schema.rs` stores images on `UserContent`, and `runtime/executor.rs::build_llm_messages_static` turns those into `ContentBlock::Image` for providers.

That is a useful template, but general file attachments should not be represented as images. We need a first-class typed attachment path so wrong states are not representable and so UI display metadata is not confused with LLM-bound content.

## Goal

Allow users to drag-and-drop files onto an existing Phoenix chat composer to attach them to the next message. Uploaded files should be written to an OS-temp-backed, conversation-scoped location on the server, and the LLM should receive enough path/metadata context to use tools to inspect those files. Image files should continue to use the existing image channel. Unsupported or oversized files should be rejected with clear inline feedback.

## Scope

### 1. Add a typed file attachment model

Introduce a non-image user attachment type, for example:

```rust
pub struct FileAttachment {
    pub original_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub stored_path: String,
}
```

Then thread it through:

- API request/response types for attachment upload and chat/create-conversation submission
- state-machine events (`Event::UserMessage`, `Event::SteerMessage`, `SteerEntry`, and any parent/sub-agent event projections that carry user messages)
- `UserContent` persistence
- queued-message/offline replay payloads on the UI

Prefer a distinct `files` field over expanding `images`, because image files have provider-native semantics while generic files should be tool-accessible server-side artifacts.

### 2. Store uploaded files in an OS temp directory

Because browsers do not expose the user's original local path for dropped files, Phoenix should upload the bytes and write them to a controlled server path under the OS temp directory, not under `~/.phoenix-ide`. Phoenix should not own long-lived cleanup logic for these attachment bytes; the operating system / temp-dir policy is responsible for eventual cleanup.

Use a conversation-scoped attachment directory rooted in `std::env::temp_dir()`, with collision-safe filenames, for example:

```text
${TMPDIR}/phoenix-ide-attachments/<conversation-id>/<uuid>-<sanitized-original-name>
```

The path must be readable by agent tools in the same workspace/container. The persisted attachment metadata should include the original filename, MIME type, size, and stored path. Enforce conservative per-file and total upload limits on both client and server.

### 3. Convert attachments into LLM-visible path context

When building LLM messages from persisted `UserContent`, append file metadata/path context, e.g.:

```text
<attached_file name="archive.zip" media_type="application/zip" size_bytes="12345" path="/tmp/phoenix-ide-attachments/conv/uuid-archive.zip" />
```

Do not inline binary bytes into the LLM message. For text-like files, the first implementation should still prefer the path as the source of truth; a bounded preview can be added later if needed. If a provider cannot consume a file natively, that is not a data drop as long as the path is present in the LLM-visible context.

### 4. Existing-chat drag-and-drop flow

Extend `InputArea` to handle file drag/drop:

- add `onDragEnter`, `onDragOver`, `onDragLeave`, and `onDrop` on the composer wrapper or textarea area
- show a compact drop affordance while files are dragged over the composer
- split dropped files into:
  - supported images → existing `processImageFiles`
  - generic files → upload via `POST /api/conversations/:id/attachments` and store returned metadata in composer state
  - unsupported/oversized files → inline warning
- preview attached files alongside image thumbnails with filename, size, and remove control
- preserve attached files on expansion errors, just like images
- clear attached files on successful send and conversation switch

The attachment upload endpoint should only write the file and return metadata. The subsequent chat POST carries `files: FileAttachment[]` metadata alongside `images`, so queued messages and retries reference already-written temp paths rather than re-uploading bytes.

### 5. New-conversation drag-and-drop flow

New-conversation attachments must not depend on a pre-existing conversation id or an ambiguous pre-upload token. Use a single multipart create request when generic files are attached:

1. The browser keeps dropped generic `File` objects in new-conversation composer state, not uploaded yet.
2. On Send, `api.createConversation` uses the existing JSON endpoint when there are no generic files.
3. On Send with generic files, `api.createConversation` sends `multipart/form-data` to `POST /api/conversations/new` (or a clearly named sibling endpoint) with:
   - one JSON metadata part containing the existing create-conversation fields (`cwd`, `model`, `text`, `message_id`, `images`, `mode`, `base_branch`, seed fields)
   - one file part per generic attachment
4. The server parses metadata, performs the same validation/worktree setup it performs today, creates the conversation id, writes each uploaded file under `${TMPDIR}/phoenix-ide-attachments/<conversation-id>/...`, then dispatches the initial `Event::UserMessage` with `images` and `files` in the same request.
5. If validation or conversation creation fails, no user message is dispatched. Any already-written temp files are left to OS temp cleanup rather than app-owned cleanup logic.

This sequence makes the ordering unambiguous: the conversation id is allocated before file storage, and file metadata is available before the initial user event is sent.

### 6. Tests

Add coverage for:

- UI file processing: dropped file uploads and returns attachment metadata; unsupported/oversized file → warning/no payload
- `InputArea` drop behavior and send/restore-on-expansion-error semantics
- new-conversation multipart create path: server creates id, stores uploaded files under that id, then dispatches initial user message with file metadata
- backend upload path sanitization, size limits, temp-root selection, and metadata response
- backend request decoding and event threading for chat and create-conversation paths
- steering queue persistence preserves files on queued messages
- `build_llm_messages_static` includes attached file paths in the LLM text while persisted/display text remains unchanged
- image attachments continue to use `ContentBlock::Image`

## Non-goals / follow-ups

- Provider-native PDF/document blocks are out of scope unless the LLM abstraction is expanded for them. Start with tool-accessible stored files plus LLM-visible paths.
- Directory drops are out of scope for the first implementation.
- Large-file chunking/summarization is out of scope; enforce a conservative per-file and total attachment size limit client-side and server-side.
- App-owned cleanup of attachment files is out of scope; temp-file lifecycle is delegated to the OS temp directory policy.

## Validation

Run:

```bash
./dev.py check
```

Also manually verify:

1. Drag a `.txt` or `.md` file onto an existing chat, preview it, send it, and confirm the model receives a path it can read.
2. Drag a `.zip` file, send it, and confirm the model receives a path it can inspect/decompress with tools.
3. Start a new conversation with a generic file attached and confirm the first user turn receives a temp path under that new conversation id.
4. Drag an image and a generic file together; image renders as image, generic file renders as file chip.
5. Drop an unsupported or oversized file and confirm it is rejected visibly.
6. Queue a message while the agent is busy and confirm the attachment path survives until delivery.
