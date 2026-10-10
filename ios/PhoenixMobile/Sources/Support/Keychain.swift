import Foundation
import Security

/// Minimal generic-password Keychain wrapper. Stores the Phoenix server
/// password; everything else (server URL, toggles) lives in UserDefaults.
enum Keychain {
    private static let service = "com.scottopell.phoenix-ide"
    static let credentialAccount = "server-credentials"
    static let legacyPasswordAccount = "server-password"

    struct CredentialRecord: Codable, Equatable {
        let version: Int
        let password: String
        let generation: UUID
        let legacyServerURL: String?

        init(
            password: String, legacyServerURL: String? = nil, generation: UUID = UUID()
        ) {
            version = 1
            self.password = password
            self.generation = generation
            self.legacyServerURL = legacyServerURL
        }

        func legacyPersistenceScope(serverURL: URL) -> String? {
            guard legacyServerURL == serverURL.absoluteString else { return nil }
            return "\(serverURL.absoluteString)|\(generation.uuidString)"
        }
    }

    enum CredentialError: Error {
        case invalidRecord
    }

    static func loadCredential(
        persistedServerURL: String,
        read: (String) throws -> String? = readPassword,
        write: (String, String) throws -> Void = { try setPassword($0, account: $1) },
        delete: (String) -> Void = { deletePassword(account: $0) }
    ) throws -> CredentialRecord {
        if let encoded = try read(credentialAccount) {
            guard let record = try? JSONDecoder().decode(CredentialRecord.self, from: Data(encoded.utf8)),
                  record.version == 1
            else { throw CredentialError.invalidRecord }
            return record
        }
        let legacyPassword = try read(legacyPasswordAccount)
        let legacyURL = URL(string: persistedServerURL)
        let provenURL = legacyPassword != nil && legacyURL?.host != nil
            ? legacyURL?.absoluteString : nil
        let record = CredentialRecord(password: legacyPassword ?? "", legacyServerURL: provenURL)
        try saveCredential(record, write: write)
        if legacyPassword != nil { delete(legacyPasswordAccount) }
        return record
    }

    static func saveCredential(
        _ record: CredentialRecord,
        write: (String, String) throws -> Void = { try setPassword($0, account: $1) }
    ) throws {
        let data = try JSONEncoder().encode(record)
        try write(String(decoding: data, as: UTF8.self), credentialAccount)
    }

    static func deleteCredential() {
        deletePassword(account: credentialAccount)
        deletePassword(account: legacyPasswordAccount)
    }

    struct StoreError: LocalizedError {
        let status: OSStatus

        var errorDescription: String? {
            "Password could not be saved securely (Keychain status \(status))."
        }
    }

    static func setPassword(_ password: String, account: String) throws {
        let data = Data(password.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let values: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        let updateStatus = SecItemUpdate(query as CFDictionary, values as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw StoreError(status: updateStatus)
        }
        var attributes = query
        values.forEach { attributes[$0.key] = $0.value }
        let addStatus = SecItemAdd(attributes as CFDictionary, nil)
        guard addStatus == errSecSuccess else { throw StoreError(status: addStatus) }
    }

    static func password(account: String) -> String? {
        try? readPassword(account)
    }

    private static func readPassword(_ account: String) throws -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw StoreError(status: status) }
        guard let data = result as? Data, let value = String(data: data, encoding: .utf8) else {
            throw CredentialError.invalidRecord
        }
        return value
    }

    static func deletePassword(account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
