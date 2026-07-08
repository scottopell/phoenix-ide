import Foundation

/// Trust-on-first-use certificate pin for the configured Phoenix server.
///
/// Phoenix deployments typically serve self-signed TLS (see TLS.md), so
/// standard chain validation fails by design. Accepting *any* certificate
/// would let a MITM on a hostile network capture the Bearer password —
/// instead, the first successful connection records the leaf certificate's
/// SHA-256 fingerprint, and every later connection must present the same
/// one. A mismatch hard-fails the connection and is surfaced in Settings,
/// where the user can explicitly forget the pin (e.g. after reinstalling
/// the server) to re-pin on the next connect.
///
/// Single-pin model: the app talks to one server at a time; pinning a new
/// host/port replaces the old pin. UserDefaults-backed and accessed from
/// URLSession delegate queues, which is safe — UserDefaults is thread-safe.
enum CertPinStore {
    private static let hostKey = "phoenix.certPin.host"
    private static let fingerprintKey = "phoenix.certPin.fingerprint"
    private static let mismatchAtKey = "phoenix.certPin.mismatchAt"

    enum Decision {
        case accept
        case reject
    }

    /// TOFU evaluation for a self-signed presentation.
    static func evaluate(host: String, port: Int, fingerprint: String) -> Decision {
        let defaults = UserDefaults.standard
        let key = "\(host):\(port)"
        let pinnedHost = defaults.string(forKey: hostKey)
        let pinnedFingerprint = defaults.string(forKey: fingerprintKey)

        guard pinnedHost == key, let pinnedFingerprint else {
            // First use (or a different server): pin and accept.
            defaults.set(key, forKey: hostKey)
            defaults.set(fingerprint, forKey: fingerprintKey)
            defaults.removeObject(forKey: mismatchAtKey)
            return .accept
        }
        if pinnedFingerprint == fingerprint {
            return .accept
        }
        // Certificate changed: fail closed and record it for Settings.
        defaults.set(Date(), forKey: mismatchAtKey)
        return .reject
    }

    /// The pinned server ("host:port") and a short fingerprint prefix for
    /// display, or nil when nothing is pinned yet.
    static var pinnedDescription: String? {
        let defaults = UserDefaults.standard
        guard let host = defaults.string(forKey: hostKey),
              let fingerprint = defaults.string(forKey: fingerprintKey)
        else { return nil }
        return "\(host) · sha256:\(fingerprint.prefix(16))…"
    }

    /// When the last pin mismatch was observed, or nil. A non-nil value
    /// means recent connections are being rejected because the server's
    /// certificate changed.
    static var lastMismatchAt: Date? {
        UserDefaults.standard.object(forKey: mismatchAtKey) as? Date
    }

    /// Explicit re-trust: forget the pin so the next connection pins the
    /// certificate the server presents then.
    static func forget() {
        let defaults = UserDefaults.standard
        defaults.removeObject(forKey: hostKey)
        defaults.removeObject(forKey: fingerprintKey)
        defaults.removeObject(forKey: mismatchAtKey)
    }
}
