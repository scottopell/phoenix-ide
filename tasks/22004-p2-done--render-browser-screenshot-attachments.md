# Render browser screenshot attachment URIs in conversation Markdown

Support agent-authored Markdown images whose source is an `attachment:///tmp/phoenix-screenshot-<uuid>.png` URI produced from browser screenshot artifacts. Rewrite only that controlled URI shape to Phoenix's preview transport, admit only the corresponding exact temporary screenshot path at the backend, and retain all existing project-image, external-image, and unsafe-scheme behavior. Add finalized/streaming UI tests, backend containment tests, and normative spec coverage.
