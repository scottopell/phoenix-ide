import SwiftUI

/// Renders one authoritative message. Content is polymorphic on the wire
/// (see ui/src/api.ts MessageContent); shapes we don't recognize fall back
/// to a compact JSON rendering rather than disappearing.
///
/// `toolIndex` joins tool result messages to the tool_use block that
/// invoked them (results carry only `tool_use_id`), so ToolResultMessageView
/// can pick a native renderer.
struct MessageView: View {
    let message: Message
    var toolIndex: [String: ToolUseRef] = [:]

    var body: some View {
        // Recovery markers (dismissed errors, answered questions) persist
        // with display_data.hidden — the web renderer skips them; so do we.
        if message.display_data?["hidden"]?.boolValue == true {
            EmptyView()
        } else {
            typedBody
        }
    }

    @ViewBuilder
    private var typedBody: some View {
        switch message.message_type {
        case "user":
            UserMessageView(content: message.content)
        case "agent":
            AgentMessageView(content: message.content)
        case "tool":
            ToolResultMessageView(content: message.content, toolUse: invokingToolUse)
        case "error":
            SystemNote(text: noteText, style: .red)
        case "system", "continuation", "skill":
            SystemNote(text: noteText, style: .secondary)
        default:
            SystemNote(text: noteText, style: .secondary)
        }
    }

    private var invokingToolUse: ToolUseRef? {
        message.content["tool_use_id"]?.stringValue.flatMap { toolIndex[$0] }
    }

    private var noteText: String {
        message.content.stringValue
            ?? message.content["text"]?.stringValue
            ?? message.content.compactDescription
    }
}

struct UserMessageView: View {
    let content: JSONValue

    var body: some View {
        HStack {
            Spacer(minLength: 40)
            VStack(alignment: .trailing, spacing: 4) {
                Text(content["text"]?.stringValue ?? content.compactDescription)
                    .font(.body)
                    .padding(10)
                    .background(Color.accentColor)
                    .foregroundStyle(.white)
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                if let images = content["images"]?.arrayValue, !images.isEmpty {
                    Label("\(images.count) image\(images.count == 1 ? "" : "s")",
                          systemImage: "photo")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// Agent content is a block array: text blocks render as prose, tool_use
/// blocks as collapsed cards.
struct AgentMessageView: View {
    let content: JSONValue

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let blocks = content.arrayValue {
                ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                    blockView(block)
                }
            } else {
                proseView(content.stringValue ?? content.compactDescription)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func blockView(_ block: JSONValue) -> some View {
        switch block["type"]?.stringValue {
        case "text":
            if let text = block["text"]?.stringValue, !text.isEmpty {
                proseView(text)
            }
        case "tool_use":
            ToolUseBlockView(block: block)  // dispatches to native renderers
        case "thinking", "redacted_thinking":
            // Deliberately suppressed: interleaved reasoning is rendered via
            // the think tool card, not raw thinking blocks.
            EmptyView()
        default:
            // Unknown block type (e.g. a newer server): degrade visibly to
            // compact JSON, never silently drop (REQ-IOS-010).
            Text(block.compactDescription)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(4)
        }
    }

    private func proseView(_ text: String) -> some View {
        // Markdown when it parses; plain text otherwise. Full fidelity
        // (code fences, tables) is a non-goal for v1.
        Group {
            if let attributed = try? AttributedString(
                markdown: text,
                options: AttributedString.MarkdownParsingOptions(
                    interpretedSyntax: .inlineOnlyPreservingWhitespace))
            {
                Text(attributed)
            } else {
                Text(text)
            }
        }
        .font(.body)
        .textSelection(.enabled)
        .padding(10)
        .background(Color(.secondarySystemBackground))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}

/// Fallback tool invocation card for tools without a native renderer (see
/// ToolViews.swift for the dispatch pattern): name + one-line input summary,
/// expandable to the full input.
struct GenericToolUseCard: View {
    let block: JSONValue
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.caption)
                    Text(block["name"]?.stringValue ?? "tool")
                        .font(.caption.monospaced().bold())
                    Text(summary)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer()
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            if expanded {
                ScrollView(.horizontal) {
                    Text(inputDetail)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
            }
        }
        .padding(8)
        .background(Color(.tertiarySystemBackground))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(Color(.separator), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private var summary: String {
        // Bash tool_use blocks carry a server-cleaned display command.
        if let display = block["display"]?.stringValue { return display }
        if let input = block["input"] {
            if let cmd = input["cmd"]?.stringValue ?? input["command"]?.stringValue {
                return cmd
            }
            return input.compactDescription
        }
        return ""
    }

    private var inputDetail: String {
        block["input"]?.compactDescription ?? ""
    }
}

/// Fallback tool result card for tools without a native renderer: status
/// line + collapsible output, error-tinted on failure.
struct GenericToolResultCard: View {
    let content: JSONValue
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: isError ? "xmark.circle.fill" : "checkmark.circle.fill")
                        .font(.caption)
                        .foregroundStyle(isError ? .red : .green)
                    Text(isError ? "tool failed" : "tool result")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(firstLine)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer()
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            if expanded {
                ScrollView(.horizontal) {
                    Text(output.isEmpty ? "(no output)" : output)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
                .frame(maxHeight: 300)
            }
        }
        .padding(8)
        .background(isError ? Color.red.opacity(0.08) : Color(.tertiarySystemBackground))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private var isError: Bool {
        content["is_error"]?.boolValue ?? (content["error"] != nil && content["error"] != .null)
    }

    private var output: String {
        content["content"]?.stringValue
            ?? content["result"]?.stringValue
            ?? content["error"]?.stringValue
            ?? content.compactDescription
    }

    private var firstLine: String {
        output.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
    }
}

struct SystemNote: View {
    let text: String
    let style: Color

    init(text: String, style: Color) {
        self.text = text
        self.style = style
    }

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(style)
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.vertical, 2)
    }
}
