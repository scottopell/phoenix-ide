browser_take_screenshot puts the screenshot's base64 PNG into the UI-only display_data blob instead of the typed images channel, so the image NEVER reaches the LLM and the drop is completely silent (no log).

Verified location: crates/phoenix-ide/src/tools/browser/tools.rs:363-373

  // Return base64 for vision
  let base64_data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data);
  ToolOutput::success(format!("Screenshot taken (saved as {path})")).with_display(json!({
      "type": "image", "media_type": "image/png", "data": base64_data,
  }))

Why egregious:
- The `// Return base64 for vision` comment is a load-bearing lie. display_data is UI-only and is structurally never threaded to the LLM. Proof chain: ToolOutput.display_data (tools.rs:73-74) -> ToolOutcome::Success{display_data,images} (runtime/executor.rs:1917-1922) -> MessageContent::tool_with_images(..., result.images()...) (executor.rs:1418-1423, only images passed) -> ContentBlock::ToolResult{images,...} (executor.rs:2204-2218, built only from ToolContent.images). display_data is dropped on the LLM path; wire.rs:618-622 confirms it is the UI-only payload.
- This is the exact "Omission is data loss" + "Capability gaps are logged, not silenced" violation: the screenshot the model is told to view never arrives, and nothing is logged.

Correct sibling pattern (codebase knows the right way):
- crates/phoenix-ide/src/tools/read_image.rs:133-147 carries base64 via .with_images(vec![ToolImage{...}]) with a comment that the payload is carried ONLY by the typed images channel and is "deliberately NOT duplicated into display_data", plus a regression test (read_image.rs:210-216) asserting display_data.is_none().

Related tasks:
- 13013-p2-done--read-image-parallel-representation: fixed read_image's duplication and its body explicitly noted "browser screenshot tool which only writes display_data" -- flagged but never fixed for the browser tool.

Fix direction: route the screenshot base64 through .with_images(...) (typed ToolImage channel) like read_image does, not display_data. display_data should hold only UI-only metadata if anything. Add a regression test mirroring read_image.rs:210-216. If a provider cannot accept images, log the drop at debug+.
