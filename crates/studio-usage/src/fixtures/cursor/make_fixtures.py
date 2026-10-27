"""Writes the Cursor fixtures: small SQLite files shaped like Cursor's
state.vscdb (VS Code's ItemTable, Cursor's cursorDiskKV), with test tokens
only. Run from anywhere: python3 -I make_fixtures.py <this folder>

- state.vscdb: rollback journal, 512-byte pages so the token overflows onto
  two overflow pages and both B-trees have interior pages; the token was
  replaced once, so a stale copy sits in freed pages.
- wal.vscdb (+ -wal, -shm): WAL mode, copied while a connection is open: the
  main file holds an older token, the WAL the newer one.
- signed-out.vscdb: default 4096-byte pages, Cursor installed, nobody signed
  in (no cursorAuth keys).

WAL salts are random, so the WAL files differ on every run.
"""

import base64
import json
import os
import shutil
import sqlite3
import sys

OUT = sys.argv[1]
SCHEMA = "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)"
DISK = "CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)"


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def jwt(exp, sig_len):
    head = b64(json.dumps({"alg": "RS256", "typ": "JWT"}, separators=(",", ":")).encode())
    claims = {
        "sub": "auth0|user_test",
        "time": "1793000000",
        "randomness": "00000000-test-test-test-000000000000",
        "exp": exp,
        "iss": "https://authentication.cursor.sh",
        "scope": "openid profile email offline_access",
        "aud": "https://cursor.com",
        "type": "session",
    }
    body = b64(json.dumps(claims, separators=(",", ":")).encode())
    sig = b64(bytes((i * 7 + 3) % 256 for i in range(sig_len)))
    return f"{head}.{body}.{sig}"


def fillers(start, count):
    rows = []
    for i in range(start, start + count):
        kind = ("workbench.view.", "memento/editor.", "extension.state.")[i % 3]
        rows.append((f"{kind}{i:03}", json.dumps({"i": i, "open": i % 2 == 0, "width": 200 + i})))
    return rows


def fresh(name):
    path = os.path.join(OUT, name)
    for suffix in ("", "-wal", "-shm", "-journal"):
        if os.path.exists(path + suffix):
            os.remove(path + suffix)
    return path


def put(con, rows):
    con.executemany("INSERT INTO ItemTable VALUES (?, ?)", rows)


def main_store():
    con = sqlite3.connect(fresh("state.vscdb"))
    con.execute("PRAGMA page_size = 512")
    con.execute("PRAGMA user_version = 1")
    con.execute(SCHEMA)
    con.execute(DISK)
    put(con, fillers(0, 120))
    put(con, [
        ("cursorAuth/accessToken", jwt(1_900_000_000, 512)),
        ("cursorAuth/refreshToken", "test-refresh-token"),
        ("cursorAuth/cachedEmail", "test@example.com"),
        ("cursorAuth/cachedSignUpType", "Auth_0"),
        ("cursorAuth/stripeMembershipType", "pro"),
    ])
    put(con, fillers(120, 120))
    con.executemany("INSERT INTO cursorDiskKV VALUES (?, ?)", [(f"composerData:{i:04}", "x" * 1500) for i in range(4)])
    # Cursor renews its token: REPLACE gives the row a new rowid and frees
    # the old cell and its overflow pages.
    put(con, [("cursorAuth/accessToken", jwt(2_000_000_000, 512))])
    con.commit()
    con.close()


def wal_store():
    path = fresh("wal.vscdb")
    con = sqlite3.connect(path)
    con.execute("PRAGMA page_size = 512")
    con.execute("PRAGMA journal_mode = WAL")
    con.execute("PRAGMA wal_autocheckpoint = 0")
    con.execute(SCHEMA)
    put(con, fillers(0, 60))
    put(con, [
        ("cursorAuth/accessToken", jwt(1_900_000_000, 256)),
        ("cursorAuth/cachedEmail", "test@example.com"),
        ("cursorAuth/stripeMembershipType", "free_trial"),
    ])
    con.commit()
    con.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    put(con, [("cursorAuth/accessToken", jwt(2_000_000_000, 256)), ("cursorAuth/stripeMembershipType", "pro")])
    con.commit()
    for suffix in ("", "-wal", "-shm"):
        shutil.copyfile(path + suffix, path + ".copy" + suffix)
    con.close()
    for suffix in ("", "-wal", "-shm"):
        os.replace(path + ".copy" + suffix, path + suffix)


def signed_out_store():
    con = sqlite3.connect(fresh("signed-out.vscdb"))
    con.execute("PRAGMA user_version = 1")
    con.execute(SCHEMA)
    put(con, fillers(0, 12) + [("cursorAuth/cachedSignUpType", "Auth_0")])
    con.commit()
    con.close()


main_store()
wal_store()
signed_out_store()
for name in sorted(os.listdir(OUT)):
    if "vscdb" in name:
        print(name, os.path.getsize(os.path.join(OUT, name)))
