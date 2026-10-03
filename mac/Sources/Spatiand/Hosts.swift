//  Hosts.swift — the computers this Mac has paired with.

import Foundation

struct PairedHost: Codable, Equatable {
    var name: String
    /// `name`, or `name:port`.
    var address: String
    /// The host's fingerprint, in full: a session talks to a host it recognises and nothing else.
    var fingerprint: String
}

enum Hosts {
    private static let key = "pairedHosts"

    static var all: [PairedHost] {
        get {
            guard let data = UserDefaults.standard.data(forKey: key),
                  let hosts = try? JSONDecoder().decode([PairedHost].self, from: data) else { return [] }
            return hosts
        }
        set {
            UserDefaults.standard.set(try? JSONEncoder().encode(newValue), forKey: key)
        }
    }

    static func add(_ host: PairedHost) {
        var hosts = all.filter { $0.fingerprint != host.fingerprint }
        hosts.append(host)
        all = hosts
    }

    static func forget(_ host: PairedHost) {
        all = all.filter { $0.fingerprint != host.fingerprint }
    }
}
