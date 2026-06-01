Migrate the plain-text viewer body (TextViewerBody rich path) onto a Pierre @pierre/diffs CodeView file item, mirroring the code viewer (PhoenixFileCodeView).

Follows the code-viewer Pierre migration. Once the plain-text path renders through a Pierre file item, the plainLargeText fallback for text becomes unnecessary (Pierre virtualizes) and can be dropped, and AnnotatableBlock can be retired if nothing else uses it.

Preserve: line notes, jump-to-line, scroll restoration, copy-all button. Reuse pierreFileMapping + PhoenixFileCodeView from the code migration.
