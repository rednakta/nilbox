
import Foundation
import Virtualization

/// Relays a VZVirtioSocketConnection to the Rust-side Unix relay socket.
///
/// Protocol:
///   1. Connect to `relayPath` (Rust is listening as server)
///   2. Write 4 bytes: destination port as big-endian UInt32
///   3. Bidirectionally copy data between the vsock connection fd and relay fd
struct VsockRelay {

    /// Write all `count` bytes from `buf` to `fd`. Returns false on error.
    private static func writeAll(_ fd: Int32, _ buf: UnsafeRawPointer, _ count: Int) -> Bool {
        var written = 0
        while written < count {
            let w = Darwin.write(fd, buf.advanced(by: written), count - written)
            if w <= 0 { return false }
            written += w
        }
        return true
    }

    static func relay(connection: VZVirtioSocketConnection,
                      port: UInt32,
                      relayPath: String,
                      relayToken: String = "") {
        // Open a Unix domain socket to the Rust relay listener
        let sockfd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard sockfd >= 0 else {
            fputs("[VMM] relay: socket() failed for port \(port)\n", stderr)
            return
        }

        // Connect to relay path
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)

        let pathBytes = relayPath.utf8CString
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
        guard pathBytes.count <= maxLen else {
            fputs("[VMM] relay: relay path too long\n", stderr)
            Darwin.close(sockfd)
            return
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { dst in
            pathBytes.withUnsafeBytes { src in
                dst.copyMemory(from: UnsafeRawBufferPointer(
                    start: src.baseAddress,
                    count: min(src.count, maxLen)
                ))
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                Darwin.connect(sockfd, sockaddrPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            fputs("[VMM] relay: connect() failed for port \(port): \(String(cString: strerror(errno)))\n", stderr)
            Darwin.close(sockfd)
            return
        }

        // Tune relay socket buffers — match vfkit's proven sizes.
        // macOS default Unix socket buffers (~8-16KB) cause writeAll() stalls
        // when relaying 65KB vsock reads through a tiny pipe.
        var sndBuf: Int32 = 1 * 1024 * 1024   // 1MB send buffer
        var rcvBuf: Int32 = 4 * 1024 * 1024   // 4MB receive buffer
        setsockopt(sockfd, SOL_SOCKET, SO_SNDBUF, &sndBuf, socklen_t(MemoryLayout<Int32>.size))
        setsockopt(sockfd, SOL_SOCKET, SO_RCVBUF, &rcvBuf, socklen_t(MemoryLayout<Int32>.size))

        // Send 32-byte auth token before port header
        if !relayToken.isEmpty {
            let tokenData = Self.dataFromHex(relayToken)
            guard tokenData.count == 32 else {
                fputs("[VMM] relay: invalid relay token length (\(tokenData.count)), expected 32\n", stderr)
                Darwin.close(sockfd)
                return
            }
            let tokenOk = tokenData.withUnsafeBytes { writeAll(sockfd, $0.baseAddress!, 32) }
            guard tokenOk else {
                fputs("[VMM] relay: failed to write auth token\n", stderr)
                Darwin.close(sockfd)
                return
            }
        }

        // Send 4-byte port header (big-endian)
        var portBE = port.bigEndian
        let headerOk = withUnsafeBytes(of: &portBE) { writeAll(sockfd, $0.baseAddress!, 4) }
        guard headerOk else {
            fputs("[VMM] relay: failed to write port header\n", stderr)
            Darwin.close(sockfd)
            return
        }

        let connFd = connection.fileDescriptor

        // Tune vsock fd buffers for higher throughput.
        var sndbuf: Int32 = 65536
        setsockopt(connFd, SOL_SOCKET, SO_SNDBUF, &sndbuf, socklen_t(MemoryLayout<Int32>.size))
        var vsockRcvBuf: Int32 = 1 * 1024 * 1024  // 1MB receive buffer
        setsockopt(connFd, SOL_SOCKET, SO_RCVBUF, &vsockRcvBuf, socklen_t(MemoryLayout<Int32>.size))

        let group = DispatchGroup()

        // vsock connection → relay socket
        group.enter()
        DispatchQueue.global(qos: .utility).async {
            withExtendedLifetime(connection) {
                var buf = [UInt8](repeating: 0, count: 65536)
                while true {
                    let n = Darwin.read(connFd, &buf, buf.count)
                    if n <= 0 { break }
                    let ok = buf.withUnsafeBytes { writeAll(sockfd, $0.baseAddress!, n) }
                    if !ok { break }
                }
                // Half-close: signal relay→vsock direction to stop reading
                Darwin.shutdown(sockfd, SHUT_WR)
            }
            group.leave()
        }

        // relay socket → vsock connection
        group.enter()
        DispatchQueue.global(qos: .utility).async {
            withExtendedLifetime(connection) {
                var buf = [UInt8](repeating: 0, count: 65536)
                while true {
                    let n = Darwin.read(sockfd, &buf, buf.count)
                    if n <= 0 { break }
                    let ok = buf.withUnsafeBytes { writeAll(connFd, $0.baseAddress!, n) }
                    if !ok { break }
                }
                // Half-close: signal vsock→relay direction to stop writing
                Darwin.shutdown(connFd, SHUT_WR)
            }
            group.leave()
        }

        // Close relay fd only after both directions are done.
        // connFd is owned by VZVirtioSocketConnection (ARC-managed).
        group.notify(queue: .global(qos: .utility)) {
            Darwin.close(sockfd)
        }
    }

    /// Convert a hex string (e.g. "aabb01...") to raw Data.
    private static func dataFromHex(_ hex: String) -> Data {
        var data = Data()
        data.reserveCapacity(hex.count / 2)
        var chars = hex.makeIterator()
        while let hi = chars.next(), let lo = chars.next() {
            guard let byte = UInt8(String([hi, lo]), radix: 16) else { return Data() }
            data.append(byte)
        }
        return data
    }
}
