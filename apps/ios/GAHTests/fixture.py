"""Local WKWebView session fixture. Run only for simulator tests, bound to loopback."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from socketserver import TCPServer


class LoopbackHTTPServer(ThreadingHTTPServer):
    """Bind without reverse DNS, which can stall hosted macOS runners before listen()."""
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name = 'localhost'
        self.server_port = self.server_address[1]


class Fixture(BaseHTTPRequestHandler):
    recovery_unavailable = False

    def do_GET(self):
        path = urlsplit(self.path).path
        print('Fixture GET', path, flush=True)
        if path == "/arm-recovery":
            Fixture.recovery_unavailable = True
        if path == "/allow-recovery":
            Fixture.recovery_unavailable = False
        if path == "/unavailable" or (path == "/recovery" and Fixture.recovery_unavailable):
            self.close_connection = True
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        if urlsplit(self.path).path == '/remember':
            self.send_header('Set-Cookie', 'gah_test=retained; HttpOnly; SameSite=Strict; Max-Age=3600; Path=/')
        self.end_headers()
        if path == "/frame":
            self.wfile.write(b"<script>try{window.webkit.messageHandlers.gahController.postMessage('scanPairingCode');parent.postMessage('frame-sent','*')}catch(e){parent.postMessage('frame-error '+e.message,'*')}</script>")
            return
        retained = urlsplit(self.path).path == '/remember' or 'gah_test=retained' in self.headers.get('Cookie', '')
        body = '<p>Session retained</p>' if retained else '<p>No test session</p>'
        self.wfile.write(('''<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1">
<title>GAH fixture</title><style>body{font:17px system-ui;padding:16px}button,label,a{display:block;margin:8px 0}button{min-height:44px;font:inherit}textarea{display:block;width:100%;box-sizing:border-box;font:inherit}</style>
<h1>GAH controller fixture</h1>''' + body + '''
<form action="/remember"><button>Remember test session</button></form>
<button onclick="scan(); statusText.textContent='Outside request sent'">Request scan outside Settings</button>
<button onclick="history.replaceState(null,'','?page=overview&page=settings');scan();statusText.textContent='Duplicate page request sent'">Request scan with duplicate page</button>
<button onclick="history.replaceState(null,'','?page=settings');this.focus()">Settings</button>
<label>Draft <textarea></textarea></label>
<button onclick="try{statusText.textContent='Frame started';requestFrame(false)}catch(e){statusText.textContent='Frame error '+e.message}">Request scan from subframe</button>
<button onclick="requestFrame(true)">Request scan from other origin</button>
<button onclick="scan()">Scan pairing QR code</button>
<a href="http://localhost:18773/recovery#pair=abcdefghijklmnopqrstuvwxyzABCDEF&server=e58dbf8c-9c0d-4bd4-b0f9-be02d42e16a8">Open test pairing server</a>
<p id="statusText"></p><script>
const statusText = document.getElementById('statusText');
const scan = () => window.webkit.messageHandlers.gahController.postMessage('scanPairingCode');
function requestFrame(otherOrigin) {
 const frame = document.createElement('iframe'); frame.hidden = true;
 const label = otherOrigin ? 'Request scan from other origin' : 'Request scan from subframe';
 window.addEventListener('message', function sent(event) {
  if (event.source !== frame.contentWindow) return;
  if (event.data !== 'frame-sent') { statusText.textContent = String(event.data); return; }
  statusText.textContent = label + ' sent'; window.removeEventListener('message', sent); frame.remove();
 });
 frame.src = (otherOrigin ? 'http://localhost:18773' : '') + '/frame'; document.body.append(frame);
}
</script>
<p id="pairing"></p><script>if(location.hash === "#pair=abcdefghijklmnopqrstuvwxyzABCDEF&server=e58dbf8c-9c0d-4bd4-b0f9-be02d42e16a8")
 document.getElementById("pairing").textContent = "Pairing fragment retained";</script>''').encode())

    def log_message(self, *_):
        pass


if __name__ == '__main__':
    # WebKit may preconnect without sending a request; keep control requests responsive.
    LoopbackHTTPServer(('127.0.0.1', 18773), Fixture).serve_forever()
