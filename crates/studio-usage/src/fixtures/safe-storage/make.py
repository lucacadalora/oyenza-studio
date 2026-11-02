"""Writes the safe-storage fixtures: bytes sealed the way Electron's
safeStorage (Chromium's OSCrypt) seals them, with a made-up password, key
and text only. Run from anywhere: python3 -I make.py <this folder>

- mac-v10.b64: "v10" + AES-128-CBC, IV 16 spaces, PKCS#7; the key is
  PBKDF2-HMAC-SHA1(password, "saltysalt", 1003 rounds, 16 bytes).
- windows-v10.txt: the AES-256 key (hex) and "v10" + 12-byte nonce +
  AES-256-GCM with the tag last (base64), for the Windows app's test.
- local-state.json: a Local State whose os_crypt.encrypted_key is
  base64("DPAPI" + b"a DPAPI blob") (not a real DPAPI blob: the readers
  only pass it on).
- cursor-sealed.vscdb, claude-config.json: Cursor's store and the Claude
  app's config.json with sealed sign-ins (Mac seal, same password);
  cursor-token.txt: the token sealed in that store.
"""

import base64
import hashlib
import json
import os
import sqlite3
import sys

from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

OUT = sys.argv[1]
PASSWORD = b"synthetic-safe-storage-password"
PLAIN = b"a test sign-in, not a real one"
WIN_KEY = bytes(range(32))
WIN_NONCE = b"test-nonce12"


def mac_seal(data):
    key = hashlib.pbkdf2_hmac("sha1", PASSWORD, b"saltysalt", 1003, 16)
    p = padding.PKCS7(128).padder()
    padded = p.update(data) + p.finalize()
    enc = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).encryptor()
    return b"v10" + enc.update(padded) + enc.finalize()


def write(name, text):
    with open(os.path.join(OUT, name), "w") as f:
        f.write(text)


write("mac-v10.b64", base64.b64encode(mac_seal(PLAIN)).decode() + "\n")
win = b"v10" + WIN_NONCE + AESGCM(WIN_KEY).encrypt(WIN_NONCE, PLAIN, None)
write("windows-v10.txt", WIN_KEY.hex() + "\n" + base64.b64encode(win).decode() + "\n")
state = {"browser": {"enabled_labs_experiments": []}, "os_crypt": {"audit_enabled": True, "encrypted_key": base64.b64encode(b"DPAPI" + b"a DPAPI blob").decode()}}
write("local-state.json", json.dumps(state, indent=2) + "\n")



def jwt(exp):
    b64 = lambda d: base64.urlsafe_b64encode(d).rstrip(b"=").decode()
    head = b64(json.dumps({"alg": "RS256", "typ": "JWT"}, separators=(",", ":")).encode())
    body = b64(json.dumps({"sub": "auth0|user_test", "exp": exp, "iss": "https://authentication.cursor.sh", "type": "session"}, separators=(",", ":")).encode())
    return f"{head}.{body}.{b64(bytes(range(64)))}"


# Cursor 3.27's store: the access token sealed, the email and plan plain.
# cursor-token.txt is the token inside, for the Windows test's answer.
write("cursor-token.txt", jwt(2_000_000_000) + "\n")
path = os.path.join(OUT, "cursor-sealed.vscdb")
if os.path.exists(path):
    os.remove(path)
con = sqlite3.connect(path)
con.execute("PRAGMA user_version = 1")
con.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
con.executemany("INSERT INTO ItemTable VALUES (?, ?)", [
    ("cursorAuth/accessToken", "cursor-safe-storage:v1:" + base64.b64encode(mac_seal(jwt(2_000_000_000).encode())).decode()),
    ("cursorAuth/refreshToken", "cursor-safe-storage:v1:" + base64.b64encode(mac_seal(b"test-refresh-token")).decode()),
    ("cursorAuth/cachedEmail", "test@example.com"),
    ("cursorAuth/stripeMembershipType", "pro"),
] + [(f"workbench.view.{i:03}", json.dumps({"i": i})) for i in range(20)])
con.commit()
con.close()

# The Claude app's config.json: its token cache sealed. Keys are
# "acct:<account>|<client>:<org>:<API host>:<scopes>"; expiresAt in ms.
# 1793080800000 = 2026-10-27 06:00Z, an hour after the tests' clock.
CLI = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
APP = "a473d7bb-17ac-43a7-abc0-a1343d7c2805"
acct = "acct:00000000-test-test-test-000000000000|"
cache = {
    f"{acct}{APP}:org-test:https://api.anthropic.com:user:inference user:file_upload user:profile": {"token": "app-token", "refreshToken": "app-refresh-token", "expiresAt": 1793084400000},
    f"{acct}{CLI}:org-test:https://api.anthropic.com:user:inference user:profile user:sessions:claude_code": {"token": "app-claude-code-token", "refreshToken": "app-refresh-token", "expiresAt": 1793080800000, "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"},
    f"{acct}{APP}:org-test:https://api.anthropic.com:user:inference user:office": {"token": "office-token", "refreshToken": "app-refresh-token", "expiresAt": 1893080800000},
    f"{acct}{CLI}:org-test:https://api-staging.anthropic.com:user:inference user:profile": {"token": "staging-token", "refreshToken": "app-refresh-token", "expiresAt": 1893080800000},
}
config = {"locale": "en-US", "oauth:tokenCacheV2": base64.b64encode(mac_seal(json.dumps(cache).encode())).decode(), "userThemeMode": "system"}
write("claude-config.json", json.dumps(config, indent=2) + "\n")

print("peanuts key", hashlib.pbkdf2_hmac("sha1", b"peanuts", b"saltysalt", 1003, 16).hex())
for name in sorted(os.listdir(OUT)):
    print(name, os.path.getsize(os.path.join(OUT, name)))
