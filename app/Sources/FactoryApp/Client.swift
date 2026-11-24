import Foundation

/// Talks to `factory serve`. Every client of the engine uses this same API.
struct APIClient {
    var base: URL
    var token: String?

    init(address: String, token: String? = nil) {
        let trimmed = address.trimmingCharacters(in: .whitespaces)
        let text = trimmed.hasSuffix("/") ? String(trimmed.dropLast()) : trimmed
        self.base = URL(string: text + "/v1/") ?? URL(string: "http://127.0.0.1:7878/v1/")!
        self.token = (token ?? "").isEmpty ? nil : token
    }

    private func request(_ path: String, method: String = "GET") -> URLRequest {
        var request = URLRequest(url: URL(string: path, relativeTo: base) ?? base)
        request.httpMethod = method
        request.timeoutInterval = 8
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    func get<T: Decodable>(_ path: String, as type: T.Type = T.self) async throws -> T {
        let (data, response) = try await URLSession.shared.data(for: request(path))
        try check(response, data)
        return try JSONDecoder.api.decode(T.self, from: data)
    }

    /// Sends a body and decodes the answer, whatever its status: a refusal
    /// from the engine comes back as data to show, not as a thrown error.
    func send<T: Decodable>(_ path: String, body: Data, json: Bool = false, as type: T.Type = T.self) async throws -> T {
        var request = self.request(path, method: "POST")
        request.httpBody = body
        request.timeoutInterval = 90
        request.setValue(json ? "application/json" : "text/plain; charset=utf-8", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await URLSession.shared.data(for: request)
        if let decoded = try? JSONDecoder.api.decode(T.self, from: data) {
            return decoded
        }
        try check(response, data)
        return try JSONDecoder.api.decode(T.self, from: data)
    }

    func post(_ path: String) async throws {
        let (data, response) = try await URLSession.shared.data(for: request(path, method: "POST"))
        try check(response, data)
    }

    private func check(_ response: URLResponse, _ data: Data) throws {
        guard let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) else { return }
        let message = (try? JSONDecoder().decode([String: String].self, from: data))?["error"]
        throw APIError(status: http.statusCode, message: message ?? "The engine answered \(http.statusCode).")
    }
}

struct APIError: LocalizedError {
    var status: Int
    var message: String
    var errorDescription: String? { message }
}
