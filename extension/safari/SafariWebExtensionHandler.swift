// The native side of Velo's Safari extension. Safari hands it the extension's native messages;
// it passes each one to the Velo app over the app's local socket and returns the app's answer.
//
// The app answers these requests as `velo-nmh` answers Chromium's and Firefox's (see
// docs/native-messaging.md). This file replaces the handler that
// `xcrun safari-web-extension-converter` generates (scripts/build-safari.sh).

import AppKit
import Foundation
import SafariServices

/// Talks to the Velo app over its socket, starting the app if it isn't running.
enum VeloClient {
    /// The app's bundle identifier, from its installer settings.
    static let appIdentifier = "io.github.hanam824.velo"

    /// How long to wait for the app to start listening after starting it.
    static let startTimeout: TimeInterval = 10

    /// The user's real home folder, not the extension's sandbox container.
    static var home: String {
        if let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir {
            return String(cString: dir)
        }
        return NSHomeDirectory()
    }

    /// The app's socket: `/tmp/velo-<hash>.sock`, named after the app's data folder as
    /// `velo_ipc::Endpoint::for_data_dir` names it (FNV-1a of the folder's path).
    static var socketPath: String {
        let dataDir = home + "/Library/Application Support/Velo"
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in dataDir.utf8 {
            hash = (hash ^ UInt64(byte)) &* 0x0100_0000_01b3
        }
        return "/tmp/" + String(format: "velo-%016llx.sock", hash)
    }

    /// The app's answer to native-messaging `request`, or an `error` answer.
    static func ask(_ request: Any) -> [String: Any] {
        let envelope: [String: Any] = ["version": 1, "type": "native", "request": request]
        guard var line = try? JSONSerialization.data(withJSONObject: envelope) else {
            return failure("the request isn't JSON")
        }
        line.append(0x0A)
        var reply = try? exchange(line)
        if reply == nil, launchApp() {
            let deadline = Date().addingTimeInterval(startTimeout)
            while reply == nil, Date() < deadline {
                Thread.sleep(forTimeInterval: 0.2)
                reply = try? exchange(line)
            }
        }
        guard let reply else { return failure("Velo isn't running and couldn't be started") }
        guard let object = try? JSONSerialization.jsonObject(with: reply) as? [String: Any] else {
            return failure("Velo's answer isn't JSON")
        }
        if object["ok"] as? Bool == true {
            return object["answer"] as? [String: Any] ?? ["type": "result", "ok": true]
        }
        return failure(object["error"] as? String ?? "Velo refused")
    }

    /// An `error` answer saying `message`.
    static func failure(_ message: String) -> [String: Any] {
        ["type": "error", "error": message]
    }

    /// Sends `line` on a new connection to the socket and returns the reply line.
    static func exchange(_ line: Data) throws -> Data {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.EIO) }
        defer { close(fd) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let path = Array(socketPath.utf8CString)
        guard path.count <= MemoryLayout.size(ofValue: address.sun_path) else {
            throw POSIXError(.ENAMETOOLONG)
        }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            path.withUnsafeBytes { buffer.copyMemory(from: $0) }
        }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { throw POSIXError(.ECONNREFUSED) }
        // Listing a video's formats asks YouTube, which can take a while.
        var timeout = timeval(tv_sec: 60, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))

        try line.withUnsafeBytes { bytes in
            var sent = 0
            while sent < bytes.count {
                let n = write(fd, bytes.baseAddress! + sent, bytes.count - sent)
                guard n > 0 else { throw POSIXError(.EPIPE) }
                sent += n
            }
        }
        var reply = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while !reply.contains(0x0A) {
            let n = read(fd, &buffer, buffer.count)
            if n <= 0 { break }
            reply.append(contentsOf: buffer[0..<n])
        }
        guard let end = reply.firstIndex(of: 0x0A) else { throw POSIXError(.EIO) }
        return reply[..<end]
    }

    /// Starts the Velo app; `false` if it isn't installed.
    static func launchApp() -> Bool {
        guard let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: appIdentifier)
        else { return false }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = false
        NSWorkspace.shared.openApplication(at: app, configuration: configuration)
        return true
    }
}

/// Receives the extension's native messages from Safari.
final class SafariWebExtensionHandler: NSObject, NSExtensionRequestHandling {
    func beginRequest(with context: NSExtensionContext) {
        let item = context.inputItems.first as? NSExtensionItem
        let message = item?.userInfo?[SFExtensionMessageKey]
        // Asking the app can wait on YouTube; don't hold Safari's thread.
        DispatchQueue.global(qos: .userInitiated).async {
            let answer = VeloClient.ask(message ?? [:])
            let response = NSExtensionItem()
            response.userInfo = [SFExtensionMessageKey: answer]
            context.completeRequest(returningItems: [response], completionHandler: nil)
        }
    }
}
