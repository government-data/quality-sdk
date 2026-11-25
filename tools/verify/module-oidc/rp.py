"""An invented module's service for tools/verify/module-oidc.sh and
module-purge.sh (decision 0009, V3 and V4): a small OIDC relying party of
the core, standard library only. Never shipped.

  GET /bi/health   200
  GET /bi/login    to the core's authorize, with PKCE and a state
  GET /bi/cb       the code exchanged with the people's client secret; the
                   id_token's claims as JSON (its signature is the core's;
                   this stand-in reads it, the check is on what it says)
  GET /bi/machine  a client_credentials token of the machine client; its
                   claims as JSON

The secrets are read from /run/secrets (Compose file secrets) at each
request, never printed or logged.
"""

import base64
import hashlib
import json
import os
import secrets
import ssl
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ISSUER = os.environ["QD_OIDC_ISSUER"]
PEOPLE = os.environ["QD_OIDC_PEOPLE_CLIENT_ID"]
MACHINE = os.environ["QD_OIDC_MACHINE_CLIENT_ID"]
PUBLIC = os.environ["QS_PUBLIC_URL"]
TLS = ssl.create_default_context(cafile="/rp/web.crt")
PENDING = {}



def secret(name):
    with open(f"/run/secrets/{name}", encoding="utf-8") as f:
        return f.read()


def discovery():
    url = ISSUER + "/.well-known/openid-configuration"
    with urllib.request.urlopen(url, context=TLS, timeout=10) as r:
        return json.load(r)


def token(form, client, name):
    basic = base64.b64encode(f"{client}:{secret(name)}".encode()).decode()
    req = urllib.request.Request(
        discovery()["token_endpoint"],
        data=urllib.parse.urlencode(form).encode(),
        headers={"Authorization": "Basic " + basic},
    )
    try:
        with urllib.request.urlopen(req, context=TLS, timeout=10) as r:
            return 200, json.load(r)
    except urllib.error.HTTPError as e:
        return e.code, {"error": e.read().decode(errors="replace")[:200]}


def claims(jwt):
    part = jwt.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))


class Handler(BaseHTTPRequestHandler):
    def answer(self, status, body, headers=()):
        data = json.dumps(body).encode()
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        query = dict(urllib.parse.parse_qsl(url.query))
        if url.path == "/bi/health":
            return self.answer(200, {"ok": True})
        if url.path == "/bi/login":
            state, verifier = secrets.token_urlsafe(16), secrets.token_urlsafe(48)
            PENDING[state] = verifier
            challenge = base64.urlsafe_b64encode(
                hashlib.sha256(verifier.encode()).digest()
            ).rstrip(b"=").decode()
            location = discovery()["authorization_endpoint"] + "?" + urllib.parse.urlencode({
                "response_type": "code", "client_id": PEOPLE, "scope": "openid",
                "redirect_uri": PUBLIC + "/bi/cb", "state": state,
                "code_challenge": challenge, "code_challenge_method": "S256",
            })
            return self.answer(302, {}, [("Location", location)])
        if url.path == "/bi/cb":
            verifier = PENDING.pop(query.get("state", ""), None)
            if verifier is None or "code" not in query:
                return self.answer(400, {"error": "no code or state", "query": query})
            status, body = token({
                "grant_type": "authorization_code", "code": query["code"],
                "redirect_uri": PUBLIC + "/bi/cb", "code_verifier": verifier,
            }, PEOPLE, "oidc-people-secret")
            if status != 200:
                return self.answer(502, {"token": status, "body": body})
            return self.answer(200, claims(body["id_token"]))
        if url.path == "/bi/machine":
            status, body = token({"grant_type": "client_credentials"}, MACHINE, "oidc-machine-secret")
            if status != 200:
                return self.answer(502, {"token": status, "body": body})
            return self.answer(200, claims(body["access_token"]))
        return self.answer(404, {})

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
