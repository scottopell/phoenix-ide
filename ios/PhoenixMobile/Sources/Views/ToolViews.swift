import SwiftUI

// Native tool rendering.
//
// The pattern: two dispatch views (`ToolUseBlockView` for tool_use blocks
// inside agent messages, `ToolResultMessageView` for tool result messages)
// switch on the tool name and route to a dedicated renderer. Every tool
// without a dedicated renderer falls back to the generic JSON cards in
// MessageViews.swift, so an unknown tool always degrades visibly rather
// than vanishing.
//
// To add a native renderer for a tool:
//   1. Add a case to `ToolUseBlockView` (and `ToolResultMessageView` if the
//      result payload deserves better than the generic card).
//   2. Consult the tool's wire shape in its spec (specs/<tool>/) and the
//      web renderer (ui/src/components/MessageComponents.tsx) before
//      inventing one.
// The result-side join relies on `ConversationSession.toolUseIndex`
// (tool_use_id -> ToolUseRef) because result messages carry only the id.

/// Dispatch for one tool_use block inside an agent message.
struct ToolUseBlockView: View {
    let block: JSONValue

    var body: some View {
        switch block["name"]?.stringValue {
        case "think":
            ThinkCard(block: block)
        case "bash":
            BashUseCard(block: block)
        default:
            GenericToolUseCard(block: block)
        }
    }
}

/// Dispatch for one tool result message. `toolUse` is the invoking block's
/// identity, or nil when the join failed (result arrived before its agent
/// message, or history was truncated) — the generic card handles that.
struct ToolResultMessageView: View {
    let content: JSONValue
    let toolUse: ToolUseRef?

    var body: some View {
        switch toolUse?.name {
        case "think":
            // The think result is a fixed boilerplate string ("Thoughts
            // recorded…") aimed at the LLM, not the user; the ThinkCard
            // already shows the substance. Suppress unless it errored.
            if isError {
                GenericToolResultCard(content: content)
            }
        case "bash":
            if let result = BashResult(resultText: resultText) {
                BashResultCard(result: result, isError: isError)
            } else {
                // Unparseable envelope (e.g. plain-text legacy output):
                // degrade to the generic card, never to nothing.
                GenericToolResultCard(content: content)
            }
        default:
            GenericToolResultCard(content: content)
        }
    }

    private var isError: Bool {
        content["is_error"]?.boolValue
            ?? (content["error"] != nil && content["error"] != .null)
    }

    private var resultText: String {
        content["content"]?.stringValue
            ?? content["result"]?.stringValue
            ?? content["error"]?.stringValue
            ?? ""
    }
}

// MARK: - think

/// A think tool call: the agent's recorded reasoning. Rendered as a quiet
/// quote block, clipped when long.
struct ThinkCard: View {
    let block: JSONValue
    @State private var expanded = false

    private var thoughts: String {
        block["input"]?["thoughts"]?.stringValue ?? ""
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Rectangle()
                .fill(Color(.systemGray4))
                .frame(width: 3)
                .clipShape(Capsule())
            VStack(alignment: .leading, spacing: 4) {
                Label("thought", systemImage: "lightbulb")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Text(thoughts)
                    .font(.callout.italic())
                    .foregroundStyle(.secondary)
                    .lineLimit(expanded ? nil : 4)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, 2)
        .contentShape(Rectangle())
        .onTapGesture {
            withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
        }
    }
}

// MARK: - bash

/// A bash tool call: the command, monospaced, prefixed with a prompt glyph.
/// Prefers the server-cleaned `display` string (cd prefix stripped) merged
/// into the block. Non-run ops (peek/wait/kill) render as `op handle`.
struct BashUseCard: View {
    let block: JSONValue
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("$")
                    .font(.caption.monospaced().bold())
                    .foregroundStyle(.green)
                Text(command)
                    .font(.caption.monospaced())
                    .lineLimit(expanded ? nil : 2)
                    .textSelection(.enabled)
                Spacer(minLength: 0)
                if let label {
                    Text(label)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.tertiarySystemBackground))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(Color(.separator), lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle())
        .onTapGesture {
            withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
        }
    }

    private var command: String {
        if let display = block["display"]?.stringValue, !display.isEmpty {
            return display
        }
        let input = block["input"]
        let op = input?["op"]?.stringValue ?? "run"
        if op == "run" {
            return input?["cmd"]?.stringValue ?? ""
        }
        let handle = input?["handle"]?.stringValue ?? ""
        return "\(op) \(handle)"
    }

    private var label: String? {
        block["input"]?["label"]?.stringValue
    }
}

/// Parsed bash tool response envelope (success shapes tagged by `status`,
/// error shapes tagged by `error` — REQ-BASH-002/003/006/008). Fields are
/// intentionally non-uniform per status; absent ones simply don't render.
struct BashResult {
    var status: String?
    var errorId: String?
    var errorMessage: String?
    var hint: String?
    var handle: String?
    var label: String?
    var exitCode: Int?
    var signalNumber: Int?
    var durationMs: Double?
    var waitedMs: Double?
    var truncatedBefore: Bool
    var outputText: String

    /// nil when the text is not a JSON object (legacy plain-text output).
    init?(resultText: String) {
        guard let data = resultText.data(using: .utf8),
              let json = try? JSONDecoder().decode(JSONValue.self, from: data),
              case .object = json
        else { return nil }

        status = json["status"]?.stringValue
        errorId = json["error"]?.stringValue
        errorMessage = json["error_message"]?.stringValue
        hint = json["hint"]?.stringValue
        handle = json["handle"]?.stringValue
        label = json["label"]?.stringValue
        exitCode = json["exit_code"]?.intValue
        signalNumber = json["signal_number"]?.intValue
        durationMs = json["duration_ms"]?.numberValue
        waitedMs = json["waited_ms"]?.numberValue
        truncatedBefore = json["truncated_before"]?.boolValue ?? false
        // `partial` is the live trailing un-newlined line (progress bars,
        // prompts); without it a command that hasn't flushed a newline
        // renders as silent.
        var text = (json["lines"]?.arrayValue ?? [])
            .compactMap { $0["bytes"]?.stringValue }
            .joined(separator: "\n")
        if let partial = json["partial"]?.stringValue, !partial.isEmpty {
            text = text.isEmpty ? partial : text + "\n" + partial
        }
        outputText = text

        guard status != nil || errorId != nil else { return nil }
    }

    var isFailure: Bool {
        if errorId != nil { return true }
        if let exitCode, exitCode != 0 { return true }
        if status == "killed" || signalNumber != nil { return true }
        return false
    }

    var isInFlight: Bool {
        status == "running" || status == "still_running" || status == "kill_pending_kernel"
    }

    /// One-glance summary: "exited 0 · 1.2s", "still running · b-1", …
    var headline: String {
        if let errorId {
            return errorId.replacingOccurrences(of: "_", with: " ")
        }
        var parts: [String] = []
        switch status {
        case "exited": parts.append("exited \(exitCode.map(String.init) ?? "?")")
        case "killed": parts.append("killed\(signalNumber.map { " (signal \($0))" } ?? "")")
        case "running": parts.append("running")
        case "still_running": parts.append("still running")
        case "kill_pending_kernel": parts.append("kill pending")
        case "tombstoned":
            parts.append("finished")
            if let exitCode { parts.append("exit \(exitCode)") }
        case "waiter_panicked": parts.append("waiter panicked")
        default: parts.append(status ?? "?")
        }
        if isInFlight, let handle { parts.append(handle) }
        if let ms = durationMs ?? waitedMs {
            parts.append(Self.formatDuration(ms))
        }
        return parts.joined(separator: " · ")
    }

    static func formatDuration(_ ms: Double) -> String {
        if ms < 1000 { return "\(Int(ms))ms" }
        if ms < 60_000 { return String(format: "%.1fs", ms / 1000) }
        return String(format: "%.1fm", ms / 60_000)
    }
}

/// Bash result: status header (colored by outcome) + collapsible monospace
/// output. Output preview shows the tail — for a failed command the error
/// is usually the last lines, not the first.
struct BashResultCard: View {
    let result: BashResult
    let isError: Bool
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    statusIcon
                    Text(result.headline)
                        .font(.caption)
                        .foregroundStyle(failed ? .red : .secondary)
                    if let label = result.label {
                        Text(label)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    if !result.outputText.isEmpty {
                        Image(systemName: expanded ? "chevron.up" : "chevron.down")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .buttonStyle(.plain)

            if let message = result.errorMessage {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
            if expanded, let hint = result.hint {
                Text(hint)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            if !result.outputText.isEmpty {
                if expanded {
                    if result.truncatedBefore {
                        Text("[output truncated before this view]")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    ScrollView(.horizontal) {
                        Text(result.outputText)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                    }
                    .frame(maxHeight: 300)
                } else {
                    Text(tailPreview)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(failed ? Color.red.opacity(0.08) : Color(.tertiarySystemBackground))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private var failed: Bool { result.isFailure || isError }

    @ViewBuilder
    private var statusIcon: some View {
        if result.isInFlight {
            Image(systemName: "circle.dotted")
                .font(.caption)
                .foregroundStyle(.orange)
        } else if failed {
            Image(systemName: "xmark.circle.fill")
                .font(.caption)
                .foregroundStyle(.red)
        } else {
            Image(systemName: "checkmark.circle.fill")
                .font(.caption)
                .foregroundStyle(.green)
        }
    }

    private var tailPreview: String {
        let lines = result.outputText.split(separator: "\n", omittingEmptySubsequences: false)
        return lines.suffix(2).joined(separator: "\n")
    }
}
