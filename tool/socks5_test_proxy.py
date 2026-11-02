# Minimal SOCKS5 proxy (no auth, CONNECT, IPv4 or domain, socks5h-style
# resolution on the proxy). Test aid only.
import socket, struct, sys, threading
def pipe(a, b):
    try:
        while True:
            d = a.recv(65536)
            if not d: break
            b.sendall(d)
    except OSError: pass
    finally:
        try: b.shutdown(socket.SHUT_WR)
        except OSError: pass
def handle(c):
    try:
        v, n = c.recv(2); c.recv(n); c.sendall(b"\x05\x00")
        v, cmd, _, atyp = c.recv(4)
        if atyp == 1: host = socket.inet_ntoa(c.recv(4))
        elif atyp == 3: host = c.recv(c.recv(1)[0]).decode()
        else: c.close(); return
        port = struct.unpack(">H", c.recv(2))[0]
        r = socket.create_connection((host, port), 10)
        c.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("0.0.0.0") + struct.pack(">H", 0))
        sys.stderr.write(f"socks5: CONNECT {host}:{port}\n"); sys.stderr.flush()
        threading.Thread(target=pipe, args=(r, c), daemon=True).start(); pipe(c, r)
    except Exception as e:
        sys.stderr.write(f"socks5: {e}\n")
    finally:
        try: c.close()
        except OSError: pass
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(16)
while True:
    conn, _ = s.accept(); threading.Thread(target=handle, args=(conn,), daemon=True).start()
