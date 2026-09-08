"""Local WKWebView session fixture. Run only for simulator tests, bound to loopback."""
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        if urlsplit(self.path).path == '/remember':
            self.send_header('Set-Cookie', 'gah_test=retained; HttpOnly; SameSite=Strict; Max-Age=3600; Path=/')
        self.end_headers()
        retained = urlsplit(self.path).path == '/remember' or 'gah_test=retained' in self.headers.get('Cookie', '')
        body = '<p>Session retained</p>' if retained else '<p>No test session</p>'
        self.wfile.write(('''<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1">
<title>GAH fixture</title><style>body{font:17px system-ui;padding:16px}button{min-height:44px}</style>
<h1>GAH controller fixture</h1>''' + body + '''
<form action="/remember"><button>Remember test session</button></form>
<label>Draft <textarea></textarea></label>''').encode())

    def log_message(self, *_):
        pass


if __name__ == '__main__':
    HTTPServer(('127.0.0.1', 18773), Fixture).serve_forever()
