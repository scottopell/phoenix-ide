Migrate the HTML viewer source mode (HtmlViewerBody source view) onto a Pierre @pierre/diffs CodeView file item, sharing PhoenixFileCodeView with the code viewer. Preview mode (sandboxed iframe) is untouched.

Follows the code-viewer Pierre migration. Source mode currently renders highlighted code via the same react-syntax-highlighter family; route it through the Pierre file item so source mode gets virtualization + unified annotation overlay, then drop its plainLargeText fallback.

Keep the source/preview toggle and Open-in-browser affordances intact.
