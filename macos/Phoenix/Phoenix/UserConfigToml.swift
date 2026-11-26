import Foundation

enum UserLLMAuthHeader: String, CaseIterable, Equatable, Sendable {
    case bearer = "bearer"
    case apiKey = "api_key"

    var label: String {
        switch self {
        case .bearer: "Authorization: Bearer"
        case .apiKey: "Provider default (API key header)"
        }
    }
}

struct UserLLMConfig: Equatable, Sendable {
    var apiKeyHelper: String?
    var apiKeyHelperTTLMs: UInt64?
    var authHeader: UserLLMAuthHeader?
    var defaultModel: String?
    var anthropicBaseURL: String?
    var openaiBaseURL: String?
    var openaiResponsesBaseURL: String?
    var openaiChatCompletionsBaseURL: String?
    var customHeaders: [String: String] = [:]
    var requestTags: [String: String] = [:]
    var requestModels: [String: [String: String]] = [:]

    var isEmpty: Bool {
        apiKeyHelper == nil
            && apiKeyHelperTTLMs == nil
            && authHeader == nil
            && defaultModel == nil
            && anthropicBaseURL == nil
            && openaiBaseURL == nil
            && openaiResponsesBaseURL == nil
            && openaiChatCompletionsBaseURL == nil
            && customHeaders.isEmpty
            && requestTags.isEmpty
            && requestModels.isEmpty
    }

    var configuredEndpoints: [String] {
        var names: [String] = []
        if anthropicBaseURL != nil { names.append("Anthropic") }
        if openaiResponsesBaseURL != nil { names.append("OpenAI Responses") }
        if openaiChatCompletionsBaseURL != nil { names.append("OpenAI Chat Completions") }
        if openaiBaseURL != nil { names.append("OpenAI") }
        return names
    }

    var endpointWarnings: [String] {
        let candidates: [(String, String?)] = [
            ("anthropic_base_url", anthropicBaseURL),
            ("openai_base_url", openaiBaseURL),
            ("openai_responses_base_url", openaiResponsesBaseURL),
            ("openai_chat_completions_base_url", openaiChatCompletionsBaseURL),
        ]
        var warnings: [String] = []
        for (field, value) in candidates {
            guard let value, let url = URL(string: value) else { continue }
            let trimmedPath = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            if trimmedPath.isEmpty {
                warnings.append("\(field) has no URL path — Phoenix uses exact endpoint URLs as-is.")
            }
        }
        return warnings
    }

    var summaryItems: [(label: String, value: String)] {
        var items: [(String, String)] = []
        if let helper = apiKeyHelper {
            items.append(("Auth helper", helper))
        }
        if !configuredEndpoints.isEmpty {
            items.append(("Endpoints", configuredEndpoints.joined(separator: ", ")))
        }
        if let authHeader {
            items.append(("Auth header", authHeader.label))
        }
        if let defaultModel {
            items.append(("Default model", defaultModel))
        }
        if !customHeaders.isEmpty {
            items.append(("Custom headers", "\(customHeaders.count) configured"))
        }
        if !requestTags.isEmpty {
            items.append(("Request tags", requestTags.keys.sorted().map { "\($0)=\(requestTags[$0]!)" }.joined(separator: ", ")))
        }
        let overrideCount = requestModels.values.reduce(0) { $0 + $1.count }
        if overrideCount > 0 {
            items.append(("Model aliases", "\(overrideCount) override\(overrideCount == 1 ? "" : "s")"))
        }
        return items
    }
}

struct ParsedPastedConfig: Equatable, Sendable {
    let llm: UserLLMConfig
    let rawModelsBlocks: [String]
    let warnings: [String]
}

enum UserConfigTomlError: LocalizedError, Equatable {
    case emptyInput
    case unsupportedVersion(Int)
    case staticAPIKeyNotAllowed(String)
    case unknownField(section: String, key: String, line: Int)
    case invalidSyntax(line: Int, detail: String)
    case invalidValue(field: String, detail: String)
    case noLLMConfiguration

    var errorDescription: String? {
        switch self {
        case .emptyInput:
            "Paste a [llm] TOML snippet to configure your auth helper and endpoints."
        case .unsupportedVersion(let v):
            "Unsupported Phoenix config version \(v); expected version = 1."
        case .staticAPIKeyNotAllowed(let key):
            "'\(key)' is not stored in config.toml. Enter static API keys in the API Keys tab so they stay in macOS Keychain."
        case .unknownField(let section, let key, let line):
            "Unknown field '\(key)' in [\(section)] on line \(line)."
        case .invalidSyntax(let line, let detail):
            "TOML syntax error on line \(line): \(detail)"
        case .invalidValue(let field, let detail):
            "Invalid value for \(field): \(detail)"
        case .noLLMConfiguration:
            "No [llm] settings were found. Include an [llm] section with at least api_key_helper or an endpoint URL."
        }
    }
}

enum TomlMultilineDelimiter: Equatable {
    case basic   // """
    case literal // '''
}

struct TomlMultilineTracker {
    private(set) var activeString: TomlMultilineDelimiter?

    var isInsideMultilineString: Bool {
        activeString != nil
    }

    mutating func consume(line: String) {
        var index = line.startIndex
        var inSingleBasic = false
        var inSingleLiteral = false
        while index < line.endIndex {
            let ch = line[index]
            if let delimiter = activeString {
                if delimiter == .basic && ch == "\\" {
                    index = line.index(after: index)
                    if index < line.endIndex { index = line.index(after: index) }
                    continue
                }
                let quoteChar: Character = delimiter == .basic ? "\"" : "'"
                if ch == quoteChar {
                    var runEnd = index
                    var runLength = 0
                    while runEnd < line.endIndex && line[runEnd] == quoteChar {
                        runEnd = line.index(after: runEnd)
                        runLength += 1
                    }
                    if runLength >= 3 {
                        activeString = nil
                        index = runEnd
                        continue
                    }
                    index = runEnd
                    continue
                }
                index = line.index(after: index)
                continue
            }
            if inSingleBasic {
                if ch == "\\" {
                    index = line.index(after: index)
                    if index < line.endIndex { index = line.index(after: index) }
                    continue
                }
                if ch == "\"" { inSingleBasic = false }
                index = line.index(after: index)
                continue
            }
            if inSingleLiteral {
                if ch == "'" { inSingleLiteral = false }
                index = line.index(after: index)
                continue
            }
            if ch == "#" {
                break
            }
            if line[index...].hasPrefix("\"\"\"") {
                activeString = .basic
                index = line.index(index, offsetBy: 3)
                continue
            }
            if line[index...].hasPrefix("'''") {
                activeString = .literal
                index = line.index(index, offsetBy: 3)
                continue
            }
            if ch == "\"" {
                inSingleBasic = true
            } else if ch == "'" {
                inSingleLiteral = true
            }
            index = line.index(after: index)
        }
    }
}

enum UserConfigTomlParser {
    private static let allowedLLMScalarKeys: Set<String> = [
        "api_key_helper",
        "api_key_helper_ttl_ms",
        "auth_header",
        "default_model",
        "anthropic_base_url",
        "openai_base_url",
        "openai_responses_base_url",
        "openai_chat_completions_base_url",
    ]
    private static let allowedRequestModelRoutes: Set<String> = [
        "anthropic",
        "openai_responses",
        "openai_chat_completions",
    ]

    static func normalizePastedSnippet(_ raw: String) -> String {
        var text = raw
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)

        // Strip outer markdown code fences (e.g. ```toml ... ``` from Slack)
        if text.hasPrefix("```") {
            var lines = text.components(separatedBy: "\n")
            if lines.count >= 2 {
                lines.removeFirst()
                if let last = lines.last, last.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    lines.removeLast()
                }
                text = lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
            }
        }

        // Normalize smart quotes mangled by Slack / macOS editors
        text = text
            .replacingOccurrences(of: "\u{201C}", with: "\"")
            .replacingOccurrences(of: "\u{201D}", with: "\"")
            .replacingOccurrences(of: "\u{2018}", with: "'")
            .replacingOccurrences(of: "\u{2019}", with: "'")

        return text
    }

    static func parseSnippet(_ raw: String) throws -> ParsedPastedConfig {
        let normalized = normalizePastedSnippet(raw)
        guard !normalized.isEmpty else {
            throw UserConfigTomlError.emptyInput
        }

        var llm = UserLLMConfig()
        var currentSection: Section = .root
        var sawExplicitSection = false
        var rawModelsBlocks: [String] = []
        var activeModelsBlock: [String]?
        var multiline = TomlMultilineTracker()

        let lines = normalized.components(separatedBy: "\n")

        func flushModelsBlock() {
            if let block = activeModelsBlock {
                let text = block.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty {
                    rawModelsBlocks.append(text)
                }
                activeModelsBlock = nil
            }
        }

        for (zeroIndex, rawLine) in lines.enumerated() {
            let lineNumber = zeroIndex + 1

            if multiline.isInsideMultilineString {
                multiline.consume(line: rawLine)
                if activeModelsBlock != nil {
                    activeModelsBlock?.append(rawLine)
                }
                continue
            }

            let cleaned = stripComment(rawLine).trimmingCharacters(in: .whitespaces)

            if activeModelsBlock != nil {
                if cleaned.hasPrefix("[[") && cleaned.hasSuffix("]]") {
                    let inner = String(cleaned.dropFirst(2).dropLast(2)).trimmingCharacters(in: .whitespaces)
                    let segments = (try? parseKeyPath(inner, line: lineNumber)) ?? [inner]
                    flushModelsBlock()
                    if segments == ["models"] {
                        activeModelsBlock = [rawLine]
                        continue
                    }
                } else if cleaned.hasPrefix("[") && cleaned.hasSuffix("]") {
                    let inner = String(cleaned.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                    let segments = (try? parseKeyPath(inner, line: lineNumber)) ?? [inner]
                    if segments.first == "models" {
                        activeModelsBlock?.append(rawLine)
                        continue
                    }
                    flushModelsBlock()
                } else {
                    multiline.consume(line: rawLine)
                    activeModelsBlock?.append(rawLine)
                    continue
                }
            }

            if cleaned.isEmpty { continue }

            if cleaned.hasPrefix("[[") {
                guard cleaned.hasSuffix("]]") else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "unterminated array table header")
                }
                let header = String(cleaned.dropFirst(2).dropLast(2)).trimmingCharacters(in: .whitespaces)
                let segments = (try? parseKeyPath(header, line: lineNumber)) ?? [header]
                sawExplicitSection = true
                if segments == ["models"] {
                    currentSection = .models
                    activeModelsBlock = [rawLine]
                    continue
                }
                currentSection = .other(header)
                continue
            }

            if cleaned.hasPrefix("[") {
                guard cleaned.hasSuffix("]") else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "unterminated table header")
                }
                let header = String(cleaned.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                sawExplicitSection = true
                currentSection = try parseSectionHeader(header, line: lineNumber)
                continue
            }

            // Skip unrelated sections (e.g. [agents.*], [tiers.*]) without rejecting
            // multiline arrays, inline tables, or multiline strings inside them.
            if case .other = currentSection {
                multiline.consume(line: rawLine)
                continue
            }

            guard let (rawKeyPath, rawValue) = splitKeyValue(cleaned) else {
                throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "expected 'key = value'")
            }

            let keySegments = try parseKeyPath(rawKeyPath, line: lineNumber)
            guard !keySegments.isEmpty else {
                throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "empty key")
            }

            if case .root = currentSection {
                if keySegments == ["version"] {
                    let version = try parseInt(rawValue, field: "version", line: lineNumber)
                    guard version == 1 else {
                        throw UserConfigTomlError.unsupportedVersion(version)
                    }
                    continue
                }
                // Allow bare [llm] snippets without an explicit [llm] header
                if !sawExplicitSection, let first = keySegments.first,
                   allowedLLMScalarKeys.contains(first) || first == "custom_headers" || first == "request_tags" || first == "request_models" || first == "anthropic_api_key" || first == "openai_api_key" {
                    try applyLLMKeyPath(keySegments, rawValue: rawValue, into: &llm, line: lineNumber)
                    continue
                }
                if keySegments.first == "llm", keySegments.count >= 2 {
                    try applyLLMKeyPath(Array(keySegments.dropFirst()), rawValue: rawValue, into: &llm, line: lineNumber)
                    continue
                }
                throw UserConfigTomlError.unknownField(section: "root", key: keySegments.joined(separator: "."), line: lineNumber)
            }

            switch currentSection {
            case .llm:
                try applyLLMKeyPath(keySegments, rawValue: rawValue, into: &llm, line: lineNumber)
            case .llmCustomHeaders:
                guard keySegments.count == 1 else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "header name must be a single key")
                }
                let key = keySegments[0].trimmingCharacters(in: .whitespacesAndNewlines)
                guard !key.isEmpty else {
                    throw UserConfigTomlError.invalidValue(field: "llm.custom_headers", detail: "header name must not be blank")
                }
                let value = try parseString(rawValue, field: "llm.custom_headers.\(key)", line: lineNumber)
                llm.customHeaders[key] = value.trimmingCharacters(in: .whitespacesAndNewlines)
            case .llmRequestTags:
                guard keySegments.count == 1 else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "tag key must be a single key")
                }
                let key = keySegments[0].trimmingCharacters(in: .whitespacesAndNewlines)
                guard !key.isEmpty else {
                    throw UserConfigTomlError.invalidValue(field: "llm.request_tags", detail: "tag key must not be blank")
                }
                let value = try parseString(rawValue, field: "llm.request_tags.\(key)", line: lineNumber)
                llm.requestTags[key] = value.trimmingCharacters(in: .whitespacesAndNewlines)
            case .llmRequestModels(let route):
                guard keySegments.count == 1 else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "model ID must be a single key (quote IDs containing '/' or '.')")
                }
                let modelID = keySegments[0].trimmingCharacters(in: .whitespacesAndNewlines)
                let target = try parseNonBlankString(rawValue, field: "llm.request_models.\(route).\(modelID)", line: lineNumber)
                var routeMap = llm.requestModels[route] ?? [:]
                routeMap[modelID] = target
                llm.requestModels[route] = routeMap
            case .llmRequestModelsRoot:
                guard keySegments.count == 2 else {
                    throw UserConfigTomlError.invalidSyntax(line: lineNumber, detail: "use [llm.request_models.<route>] for route overrides")
                }
                let route = keySegments[0]
                guard allowedRequestModelRoutes.contains(route) else {
                    throw UserConfigTomlError.invalidValue(field: "llm.request_models", detail: "unknown route '\(route)'")
                }
                let modelID = keySegments[1]
                let target = try parseNonBlankString(rawValue, field: "llm.request_models.\(route).\(modelID)", line: lineNumber)
                var routeMap = llm.requestModels[route] ?? [:]
                routeMap[modelID] = target
                llm.requestModels[route] = routeMap
            case .root, .models, .other:
                continue
            }
        }

        flushModelsBlock()

        guard !llm.isEmpty || !rawModelsBlocks.isEmpty else {
            throw UserConfigTomlError.noLLMConfiguration
        }

        return ParsedPastedConfig(
            llm: llm,
            rawModelsBlocks: rawModelsBlocks,
            warnings: llm.endpointWarnings
        )
    }

    static func serializeLLMBlock(_ llm: UserLLMConfig) -> String {
        guard !llm.isEmpty else { return "" }
        var lines: [String] = ["[llm]"]
        if let helper = llm.apiKeyHelper {
            lines.append("api_key_helper = \(quoteString(helper))")
        }
        if let ttl = llm.apiKeyHelperTTLMs {
            lines.append("api_key_helper_ttl_ms = \(ttl)")
        }
        if let authHeader = llm.authHeader {
            lines.append("auth_header = \(quoteString(authHeader.rawValue))")
        }
        if let defaultModel = llm.defaultModel {
            lines.append("default_model = \(quoteString(defaultModel))")
        }
        if let url = llm.anthropicBaseURL {
            lines.append("anthropic_base_url = \(quoteString(url))")
        }
        if let url = llm.openaiBaseURL {
            lines.append("openai_base_url = \(quoteString(url))")
        }
        if let url = llm.openaiResponsesBaseURL {
            lines.append("openai_responses_base_url = \(quoteString(url))")
        }
        if let url = llm.openaiChatCompletionsBaseURL {
            lines.append("openai_chat_completions_base_url = \(quoteString(url))")
        }

        if !llm.customHeaders.isEmpty {
            lines.append("")
            lines.append("[llm.custom_headers]")
            for key in llm.customHeaders.keys.sorted() {
                lines.append("\(formatKey(key)) = \(quoteString(llm.customHeaders[key]!))")
            }
        }
        if !llm.requestTags.isEmpty {
            lines.append("")
            lines.append("[llm.request_tags]")
            for key in llm.requestTags.keys.sorted() {
                lines.append("\(formatKey(key)) = \(quoteString(llm.requestTags[key]!))")
            }
        }
        for route in llm.requestModels.keys.sorted() {
            guard let mapping = llm.requestModels[route], !mapping.isEmpty else { continue }
            lines.append("")
            lines.append("[llm.request_models.\(formatKey(route))]")
            for modelID in mapping.keys.sorted() {
                lines.append("\(formatKey(modelID)) = \(quoteString(mapping[modelID]!))")
            }
        }
        return lines.joined(separator: "\n") + "\n"
    }

    private enum Section {
        case root
        case llm
        case llmCustomHeaders
        case llmRequestTags
        case llmRequestModelsRoot
        case llmRequestModels(String)
        case models
        case other(String)
    }

    private static func parseSectionHeader(_ header: String, line: Int) throws -> Section {
        let segments = try parseKeyPath(header, line: line)
        guard let first = segments.first else {
            throw UserConfigTomlError.invalidSyntax(line: line, detail: "empty section header")
        }
        if first != "llm" {
            return .other(header)
        }
        switch segments.count {
        case 1:
            return .llm
        case 2:
            switch segments[1] {
            case "custom_headers": return .llmCustomHeaders
            case "request_tags": return .llmRequestTags
            case "request_models": return .llmRequestModelsRoot
            default:
                throw UserConfigTomlError.unknownField(section: "llm", key: segments[1], line: line)
            }
        case 3 where segments[1] == "request_models":
            let route = segments[2]
            guard allowedRequestModelRoutes.contains(route) else {
                throw UserConfigTomlError.invalidValue(field: "llm.request_models", detail: "unknown route '\(route)'")
            }
            return .llmRequestModels(route)
        default:
            throw UserConfigTomlError.unknownField(section: "llm", key: segments.dropFirst().joined(separator: "."), line: line)
        }
    }

    private static func applyLLMKeyPath(
        _ keySegments: [String],
        rawValue: String,
        into llm: inout UserLLMConfig,
        line: Int
    ) throws {
        guard let first = keySegments.first else { return }
        if first == "anthropic_api_key" || first == "openai_api_key" {
            throw UserConfigTomlError.staticAPIKeyNotAllowed(first)
        }

        if first == "custom_headers" {
            if keySegments.count == 1 {
                let table = try parseInlineStringTable(rawValue, field: "llm.custom_headers", line: line)
                for (k, v) in table { llm.customHeaders[k] = v }
                return
            }
            guard keySegments.count == 2 else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "invalid custom_headers key")
            }
            let value = try parseString(rawValue, field: "llm.custom_headers.\(keySegments[1])", line: line)
            llm.customHeaders[keySegments[1]] = value.trimmingCharacters(in: .whitespacesAndNewlines)
            return
        }

        if first == "request_tags" {
            if keySegments.count == 1 {
                let table = try parseInlineStringTable(rawValue, field: "llm.request_tags", line: line)
                for (k, v) in table { llm.requestTags[k] = v }
                return
            }
            guard keySegments.count == 2 else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "invalid request_tags key")
            }
            let value = try parseString(rawValue, field: "llm.request_tags.\(keySegments[1])", line: line)
            llm.requestTags[keySegments[1]] = value.trimmingCharacters(in: .whitespacesAndNewlines)
            return
        }

        if first == "request_models" {
            if keySegments.count == 2 {
                let route = keySegments[1]
                guard allowedRequestModelRoutes.contains(route) else {
                    throw UserConfigTomlError.invalidValue(field: "llm.request_models", detail: "unknown route '\(route)'")
                }
                let table = try parseInlineStringTable(rawValue, field: "llm.request_models.\(route)", line: line)
                var routeMap = llm.requestModels[route] ?? [:]
                for (k, v) in table {
                    let trimmed = v.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !trimmed.isEmpty else {
                        throw UserConfigTomlError.invalidValue(field: "llm.request_models.\(route).\(k)", detail: "must not be blank")
                    }
                    routeMap[k] = trimmed
                }
                llm.requestModels[route] = routeMap
                return
            }
            if keySegments.count == 3 {
                let route = keySegments[1]
                guard allowedRequestModelRoutes.contains(route) else {
                    throw UserConfigTomlError.invalidValue(field: "llm.request_models", detail: "unknown route '\(route)'")
                }
                let target = try parseNonBlankString(rawValue, field: "llm.request_models.\(route).\(keySegments[2])", line: line)
                var routeMap = llm.requestModels[route] ?? [:]
                routeMap[keySegments[2]] = target
                llm.requestModels[route] = routeMap
                return
            }
            throw UserConfigTomlError.invalidSyntax(line: line, detail: "invalid request_models key")
        }

        guard keySegments.count == 1 else {
            throw UserConfigTomlError.unknownField(section: "llm", key: keySegments.joined(separator: "."), line: line)
        }

        switch first {
        case "api_key_helper":
            llm.apiKeyHelper = try parseNonBlankString(rawValue, field: "llm.api_key_helper", line: line)
        case "api_key_helper_ttl_ms":
            let ttl = try parseInt(rawValue, field: "llm.api_key_helper_ttl_ms", line: line)
            guard ttl > 0 else {
                throw UserConfigTomlError.invalidValue(field: "llm.api_key_helper_ttl_ms", detail: "must be greater than 0")
            }
            llm.apiKeyHelperTTLMs = UInt64(ttl)
        case "auth_header":
            let value = try parseNonBlankString(rawValue, field: "llm.auth_header", line: line)
            guard let parsed = UserLLMAuthHeader(rawValue: value.lowercased()) else {
                throw UserConfigTomlError.invalidValue(field: "llm.auth_header", detail: "expected \"bearer\" or \"api_key\"")
            }
            llm.authHeader = parsed
        case "default_model":
            llm.defaultModel = try parseNonBlankString(rawValue, field: "llm.default_model", line: line)
        case "anthropic_base_url":
            llm.anthropicBaseURL = try parseEndpointURL(rawValue, field: "llm.anthropic_base_url", line: line)
        case "openai_base_url":
            llm.openaiBaseURL = try parseEndpointURL(rawValue, field: "llm.openai_base_url", line: line)
        case "openai_responses_base_url":
            llm.openaiResponsesBaseURL = try parseEndpointURL(rawValue, field: "llm.openai_responses_base_url", line: line)
        case "openai_chat_completions_base_url":
            llm.openaiChatCompletionsBaseURL = try parseEndpointURL(rawValue, field: "llm.openai_chat_completions_base_url", line: line)
        default:
            throw UserConfigTomlError.unknownField(section: "llm", key: first, line: line)
        }
    }

    private static func parseEndpointURL(_ rawValue: String, field: String, line: Int) throws -> String {
        let value = try parseNonBlankString(rawValue, field: field, line: line)
        guard let components = URLComponents(string: value),
              let scheme = components.scheme?.lowercased(),
              scheme == "http" || scheme == "https",
              components.host?.isEmpty == false else {
            throw UserConfigTomlError.invalidValue(field: field, detail: "must be a valid http:// or https:// URL")
        }
        return value
    }

    private static func parseNonBlankString(_ rawValue: String, field: String, line: Int) throws -> String {
        let value = try parseString(rawValue, field: field, line: line)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else {
            throw UserConfigTomlError.invalidValue(field: field, detail: "must not be blank")
        }
        return value
    }

    private static func parseInt(_ rawValue: String, field: String, line: Int) throws -> Int {
        let cleaned = rawValue.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "_", with: "")
        guard let value = Int(cleaned) else {
            throw UserConfigTomlError.invalidValue(field: field, detail: "expected an integer on line \(line)")
        }
        return value
    }

    private static func parseString(_ rawValue: String, field: String, line: Int) throws -> String {
        let trimmed = rawValue.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("'") {
            guard trimmed.count >= 2, trimmed.hasSuffix("'") else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "unterminated single-quoted string for \(field)")
            }
            return String(trimmed.dropFirst().dropLast())
        }
        guard trimmed.hasPrefix("\""), trimmed.count >= 2, trimmed.hasSuffix("\"") else {
            throw UserConfigTomlError.invalidSyntax(line: line, detail: "expected quoted string for \(field)")
        }
        var result = ""
        let inner = trimmed.dropFirst().dropLast()
        var iterator = inner.makeIterator()
        while let ch = iterator.next() {
            if ch == "\\" {
                guard let escaped = iterator.next() else {
                    throw UserConfigTomlError.invalidSyntax(line: line, detail: "dangling escape in \(field)")
                }
                switch escaped {
                case "\"": result.append("\"")
                case "\\": result.append("\\")
                case "n": result.append("\n")
                case "r": result.append("\r")
                case "t": result.append("\t")
                default:
                    throw UserConfigTomlError.invalidSyntax(line: line, detail: "unsupported escape \\\(escaped) in \(field)")
                }
            } else {
                result.append(ch)
            }
        }
        return result
    }

    private static func parseInlineStringTable(_ rawValue: String, field: String, line: Int) throws -> [String: String] {
        let trimmed = rawValue.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix("{"), trimmed.hasSuffix("}") else {
            throw UserConfigTomlError.invalidSyntax(line: line, detail: "expected inline table '{ ... }' for \(field)")
        }
        let body = String(trimmed.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
        if body.isEmpty { return [:] }
        var result: [String: String] = [:]
        for entry in splitTopLevel(body, delimiter: ",") {
            let trimmedEntry = entry.trimmingCharacters(in: .whitespaces)
            if trimmedEntry.isEmpty { continue }
            guard let (rawKey, rawVal) = splitKeyValue(trimmedEntry) else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "expected 'key = value' inside \(field)")
            }
            let segments = try parseKeyPath(rawKey, line: line)
            guard segments.count == 1 else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "nested keys unsupported inside \(field)")
            }
            let key = segments[0].trimmingCharacters(in: .whitespacesAndNewlines)
            guard !key.isEmpty else {
                throw UserConfigTomlError.invalidValue(field: field, detail: "key must not be blank")
            }
            let val = try parseString(rawVal, field: "\(field).\(key)", line: line)
            result[key] = val.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return result
    }

    static func stripComment(_ line: String) -> String {
        var inDouble = false
        var inSingle = false
        var escaping = false
        for index in line.indices {
            let ch = line[index]
            if escaping {
                escaping = false
                continue
            }
            if inDouble {
                if ch == "\\" { escaping = true }
                else if ch == "\"" { inDouble = false }
            } else if inSingle {
                if ch == "'" { inSingle = false }
            } else {
                if ch == "\"" { inDouble = true }
                else if ch == "'" { inSingle = true }
                else if ch == "#" { return String(line[..<index]) }
            }
        }
        return line
    }

    static func splitKeyValue(_ line: String) -> (String, String)? {
        var inDouble = false
        var inSingle = false
        var escaping = false
        for index in line.indices {
            let ch = line[index]
            if escaping {
                escaping = false
                continue
            }
            if inDouble {
                if ch == "\\" { escaping = true }
                else if ch == "\"" { inDouble = false }
            } else if inSingle {
                if ch == "'" { inSingle = false }
            } else {
                if ch == "\"" { inDouble = true }
                else if ch == "'" { inSingle = true }
                else if ch == "=" {
                    let key = String(line[..<index]).trimmingCharacters(in: .whitespaces)
                    let value = String(line[line.index(after: index)...]).trimmingCharacters(in: .whitespaces)
                    return (key, value)
                }
            }
        }
        return nil
    }

    private static func splitTopLevel(_ text: String, delimiter: Character) -> [String] {
        var parts: [String] = []
        var current = ""
        var inDouble = false
        var inSingle = false
        var escaping = false
        var braceDepth = 0
        for ch in text {
            if escaping {
                current.append(ch)
                escaping = false
                continue
            }
            if inDouble {
                current.append(ch)
                if ch == "\\" { escaping = true }
                else if ch == "\"" { inDouble = false }
            } else if inSingle {
                current.append(ch)
                if ch == "'" { inSingle = false }
            } else {
                if ch == "\"" {
                    inDouble = true
                    current.append(ch)
                } else if ch == "'" {
                    inSingle = true
                    current.append(ch)
                } else if ch == "{" {
                    braceDepth += 1
                    current.append(ch)
                } else if ch == "}" {
                    braceDepth = max(0, braceDepth - 1)
                    current.append(ch)
                } else if ch == delimiter && braceDepth == 0 {
                    parts.append(current)
                    current = ""
                } else {
                    current.append(ch)
                }
            }
        }
        parts.append(current)
        return parts
    }

    static func parseKeyPath(_ raw: String, line: Int) throws -> [String] {
        let parts = splitTopLevel(raw, delimiter: ".")
        var result: [String] = []
        for part in parts {
            let trimmed = part.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty else {
                throw UserConfigTomlError.invalidSyntax(line: line, detail: "empty key segment")
            }
            if trimmed.hasPrefix("\"") || trimmed.hasPrefix("'") {
                result.append(try parseString(trimmed, field: "key", line: line))
            } else {
                result.append(trimmed)
            }
        }
        return result
    }

    private static func formatKey(_ key: String) -> String {
        let isBare = !key.isEmpty && key.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_" || $0 == "-") }
        return isBare ? key : quoteString(key)
    }

    private static func quoteString(_ value: String) -> String {
        var out = "\""
        for ch in value {
            switch ch {
            case "\\": out += "\\\\"
            case "\"": out += "\\\""
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default: out.append(ch)
            }
        }
        out.append("\"")
        return out
    }
}

enum UserConfigStore {
    static func configURL(
        inherited: [String: String] = ProcessInfo.processInfo.environment,
        userHome: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> URL {
        if let xdg = inherited["XDG_CONFIG_HOME"], !xdg.isEmpty, xdg.hasPrefix("/") {
            return URL(fileURLWithPath: xdg, isDirectory: true)
                .appendingPathComponent("phoenix-ide", isDirectory: true)
                .appendingPathComponent("config.toml")
        }
        return userHome
            .appendingPathComponent(".config", isDirectory: true)
            .appendingPathComponent("phoenix-ide", isDirectory: true)
            .appendingPathComponent("config.toml")
    }

    static func loadExistingLLMConfig(at url: URL = configURL()) throws -> UserLLMConfig? {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let content = try String(contentsOf: url, encoding: .utf8)
        do {
            let parsed = try UserConfigTomlParser.parseSnippet(content)
            return parsed.llm.isEmpty ? nil : parsed.llm
        } catch UserConfigTomlError.noLLMConfiguration {
            return nil
        }
    }

    static func mergeAndWrite(
        llm: UserLLMConfig?,
        rawModelsBlocks: [String]? = nil,
        to url: URL = configURL()
    ) throws {
        let existing = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        let merged = mergeDocument(existing: existing, llm: llm, rawModelsBlocks: rawModelsBlocks)
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data(merged.utf8).write(to: url, options: .atomic)
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    static func mergeDocument(
        existing: String,
        llm: UserLLMConfig?,
        rawModelsBlocks: [String]? = nil
    ) -> String {
        let replaceModels = rawModelsBlocks.map { !$0.isEmpty } ?? false
        let lines = existing
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .components(separatedBy: "\n")

        var preservedHeaderLines: [String] = []
        var preservedOtherSections: [String] = []
        var hasVersionLine = false

        enum KeepMode {
            case root
            case keep
            case drop
        }

        var mode: KeepMode = .root
        var droppingRootAssignment = false
        var multiline = TomlMultilineTracker()

        for (zeroIndex, line) in lines.enumerated() {
            let lineNumber = zeroIndex + 1
            if multiline.isInsideMultilineString {
                multiline.consume(line: line)
                if droppingRootAssignment {
                    if !multiline.isInsideMultilineString {
                        droppingRootAssignment = false
                    }
                    continue
                }
                switch mode {
                case .root: preservedHeaderLines.append(line)
                case .keep: preservedOtherSections.append(line)
                case .drop: break
                }
                continue
            }
            droppingRootAssignment = false

            let cleaned = UserConfigTomlParser.stripComment(line).trimmingCharacters(in: .whitespaces)
            if cleaned.hasPrefix("[[") && cleaned.hasSuffix("]]") {
                let inner = String(cleaned.dropFirst(2).dropLast(2)).trimmingCharacters(in: .whitespaces)
                let segments = (try? UserConfigTomlParser.parseKeyPath(inner, line: lineNumber)) ?? [inner]
                if replaceModels && segments == ["models"] {
                    mode = .drop
                } else {
                    mode = .keep
                    preservedOtherSections.append(line)
                }
                continue
            }
            if cleaned.hasPrefix("[") && cleaned.hasSuffix("]") {
                let inner = String(cleaned.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                let segments = (try? UserConfigTomlParser.parseKeyPath(inner, line: lineNumber)) ?? [inner]
                if segments.first == "llm" {
                    mode = .drop
                } else if replaceModels && segments.first == "models" {
                    mode = .drop
                } else {
                    mode = .keep
                    preservedOtherSections.append(line)
                }
                continue
            }

            multiline.consume(line: line)

            switch mode {
            case .root:
                if let (rawKey, _) = UserConfigTomlParser.splitKeyValue(cleaned),
                   let segments = try? UserConfigTomlParser.parseKeyPath(rawKey, line: lineNumber) {
                    if segments == ["version"] {
                        hasVersionLine = true
                    } else if segments.first == "llm" || (replaceModels && segments.first == "models") {
                        if multiline.isInsideMultilineString {
                            droppingRootAssignment = true
                        }
                        continue
                    }
                }
                preservedHeaderLines.append(line)
            case .keep:
                preservedOtherSections.append(line)
            case .drop:
                continue
            }
        }

        var chunks: [String] = []
        let headerText = preservedHeaderLines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        if hasVersionLine {
            chunks.append(headerText)
        } else if headerText.isEmpty {
            chunks.append("version = 1")
        } else {
            chunks.append("version = 1\n\n" + headerText)
        }

        if let llm, !llm.isEmpty {
            chunks.append(UserConfigTomlParser.serializeLLMBlock(llm).trimmingCharacters(in: .whitespacesAndNewlines))
        }

        if let blocks = rawModelsBlocks, !blocks.isEmpty {
            chunks.append(blocks.joined(separator: "\n\n").trimmingCharacters(in: .whitespacesAndNewlines))
        }

        let otherText = preservedOtherSections.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        if !otherText.isEmpty {
            chunks.append(otherText)
        }

        return chunks.filter { !$0.isEmpty }.joined(separator: "\n\n") + "\n"
    }
}
