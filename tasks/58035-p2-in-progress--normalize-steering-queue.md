Normalize the `conversations.steering_queue` JSON-TEXT column into child tables,
continuing the JSON-in-TEXT audit (follows the message-attachments normalization
in #310). The queue is a FIFO of pending user messages; each `SteerEntry` has its
own nested child collections (images, files) plus a skill_invocation sub-struct —
all child collections that never belong in a blob.

Target schema:
- steering_messages(message_id PK, conversation_id FK cascade, ordinal,
  text, llm_text, user_agent, skill_name, skill_body, skill_dir,
  UNIQUE(conversation_id, ordinal), CHECK skill trio all-or-nothing)
- steering_message_files(message_id FK cascade, file_ordinal, ...)
- steering_message_images(message_id FK cascade, image_ordinal, ...)

Cutover: update_steering_queue = replace-all (DELETE + re-INSERT in a tx);
remove_steering_entries = DELETE WHERE message_id IN (...); reads via a dedicated
get_steering_queue(conv_id). Drop the steering_queue column and the
SteeringQueueEnvelope persistence shim. Repoint the attachment GC sweep at
steering_message_files (removing the last `LIKE '%stored_path%'` blob scan).

Phases mirror #310: (1) tables + json_each backfill (inert); (2) cutover +
column drop; (3) GC repoint.
