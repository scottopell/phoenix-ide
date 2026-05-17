read_image tool writes the same base64 image payload into two parallel representations: the typed ToolImage channel (.with_images) and the free-form display_data JSON blob (.with_display), with an explicit base64_data.clone().

Location: crates/phoenix-ide/src/tools/read_image.rs:133-147

This is the exact anti-pattern AGENTS.md cites verbatim as canonical-bad ("same image bytes in both display_data[\"data\"] and images[0].data"). The two representations persist independently (ToolContent.images vs messages.display_data) and can diverge. The comment near line 133 claiming image data goes via the typed channel directly contradicts the .with_display call below it.

Fix direction: display_data should hold only UI-only metadata (e.g. dimensions/thumbnail ref), not the full base64 payload. The typed images channel is the single source of truth for LLM-bound image bytes. Remove the duplicated payload from display_data and update the contradicting comment. See also the related structural seam (task for ToolOutcome display_data/images split) and browser screenshot tool which only writes display_data.
