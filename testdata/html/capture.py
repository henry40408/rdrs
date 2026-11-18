"""Captures HTML goldens from the Rust rdrs server.

Usage: python3 -I testdata/html/capture.py <rdrs-binary> <workdir> <out-dir>

Starts the binary twice against a scratch SQLite DB: once with no accounts (the
setup and login pages), then after applying testdata/html/seed.sql (everything
else). Responses are written verbatim except for four normalizations, so the
Go port can compare byte-for-byte after applying the same substitutions:

  @VERSION@  the build version embedded in asset URLs
  @HEX64@    64-hex CSRF tokens on anonymous pages (derived from a random token)
  @EXPIRES@  the seeded session expiry, to the second (both "T" and " " forms)
  @NOW@      capture-time values: the list snapshot instant and "Last active"

Rust-only tooling: deleted together with the Rust code in the cutover step.
"""

import hashlib
import hmac
import http.client
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone

SECRET = "golden-secret-not-base64-0123456789"  # "-" keeps it out of the base64 decode path
PORT = 18931
HERE = os.path.dirname(os.path.abspath(__file__))


def tag_hex(domain, value):
    return hmac.new(SECRET.encode(), domain + value.encode(), hashlib.sha256).hexdigest()


def session_cookie(token):
    return f"session_token={token}.{tag_hex(b'session:', token)}"


def start_server(db_path, log_path):
    env = {
        "PATH": os.environ["PATH"],
        "DATABASE_URL": db_path,
        "RDRS_SERVER_BIND": f"127.0.0.1:{PORT}",
        "RDRS_MULTI_USER_ENABLED": "true",
        "RDRS_SECRET": SECRET,
        "RUST_LOG": "warn",
        "NO_COLOR": "1",
        "RDRS_FAST_HASH": "1",
        "RDRS_LOGIN_RATE_LIMIT_ATTEMPTS": "0",
        "RDRS_DISABLE_SIDEBAR_CACHE": "1",
    }
    proc = subprocess.Popen(
        [sys.argv[1]], env=env, stdout=open(log_path, "ab"), stderr=subprocess.STDOUT
    )
    for _ in range(100):
        try:
            c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=2)
            c.request("GET", "/health")
            if c.getresponse().status == 200:
                return proc
        except OSError:
            time.sleep(0.2)
    proc.kill()
    raise SystemExit("server did not start")


def stop_server(proc):
    proc.terminate()
    proc.wait(timeout=20)


def request(method, path, cookie=None, body=None, headers=None):
    c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=20)
    h = {"Accept-Encoding": "identity"}
    if cookie:
        h["Cookie"] = cookie
    if body is not None:
        h["Content-Type"] = "application/x-www-form-urlencoded"
    h.update(headers or {})
    c.request(method, path, body=body, headers=h)
    r = c.getresponse()
    data = r.read()
    return r.status, r, data


def normalize(data, version, anonymous, expires):
    text = data.decode("utf-8")
    if version:
        text = text.replace(version, "@VERSION@")
    if anonymous:
        text = re.sub(r"[0-9a-f]{64}", "@HEX64@", text)
    text = re.sub(r'data-snapshot-at="[^"]*"', 'data-snapshot-at="@NOW@"', text)
    text = re.sub(r'name="snapshot" value="[^"]*"', 'name="snapshot" value="@NOW@"', text)
    text = re.sub(r'(<dt>Last active</dt><dd><time datetime=")[^"]*(" data-local-text>)[^<]*',
                  r"\1@NOW@\2@NOW@", text)
    for form in (expires[:19], expires[:10] + " " + expires[11:19]):
        text = text.replace(form, "@EXPIRES@")
    return text.encode("utf-8")


def find_version(data):
    m = re.search(r"\?v=([^\"'&<> ]+)", data.decode("utf-8"))
    return m.group(1) if m else None


class Capture:
    def __init__(self, out_dir, expires):
        self.out_dir = out_dir
        self.index = []
        self.version = None
        self.expires = expires

    def take(self, name, method, path, user=None, anonymous=False, body=None, headers=None):
        cookie = session_cookie(user) if user else None
        if method == "POST" and anonymous:
            cookie, body = anonymous_form(path, body)
        status, resp, data = request(method, path, cookie, body, headers)
        if self.version is None:
            self.version = find_version(data)
        ctype = resp.getheader("Content-Type", "")
        ext = "json" if "json" in ctype else "html"
        entry = {
            "name": name, "method": method, "path": path, "user": user, "status": status,
            "content_type": ctype, "location": resp.getheader("Location"),
            "anonymous": anonymous, "body": re.sub(r"[0-9a-f]{64}", "@HEX64@", body or "") or None,
            "file": None,
        }
        if data:
            fname = f"{name}.{ext}"
            entry["file"] = fname
            with open(os.path.join(self.out_dir, fname), "wb") as fh:
                fh.write(normalize(data, self.version, anonymous, self.expires))
        self.index.append(entry)


def anonymous_form(path, body):
    """Cookie and body carrying the CSRF token the server issues with the form page."""
    _, resp, data = request("GET", path)
    cookie = next(v for k, v in resp.getheaders() if k.lower() == "set-cookie" and v.startswith("session_token="))
    csrf = re.search(rb'name="_csrf" value="([0-9a-f]+)"', data).group(1).decode()
    return cookie.split(";")[0], f"_csrf={csrf}&{body}"


def apply_seed(db_path, expires):
    invite_hash = tag_hex(b"invite:", "golden-invite-token")
    sql = open(os.path.join(HERE, "seed.sql"), encoding="utf-8").read()
    sql = sql.replace("@SESSION_EXPIRES@", expires).replace("@INVITE_HASH@", invite_hash)
    con = sqlite3.connect(db_path)
    con.executescript(sql)
    con.commit()
    con.close()


ADMIN = "golden-admin-session"
ALICE = "golden-alice-session"

# (name, method, path, user)
AUTHED_PAGES = [
    ("home", "GET", "/", ADMIN),
    ("entries", "GET", "/entries", ADMIN),
    ("entries_fragment", "GET", "/entries?fragment=1", ADMIN),
    ("entries_read", "GET", "/entries/read", ADMIN),
    ("entries_starred", "GET", "/entries/starred", ADMIN),
    ("entries_summarized", "GET", "/entries/summarized", ADMIN),
    ("entries_today", "GET", "/entries/today", ADMIN),
    ("entries_offline", "GET", "/entries/offline", ALICE),
    ("entry_redirect", "GET", "/entries/1", ADMIN),
    ("entries_open_entry", "GET", "/entries?entry=1", ADMIN),
    ("entry_fragment_1", "GET", "/entries/1/fragment", ADMIN),
    ("entry_fragment_3", "GET", "/entries/3/fragment", ADMIN),
    ("entry_fragment_4_full", "GET", "/entries/4/fragment", ADMIN),
    ("entry_fragment_6_nolink", "GET", "/entries/6/fragment", ADMIN),
    ("entry_fragment_7_untitled", "GET", "/entries/7/fragment", ADMIN),
    ("entry_fragment_alice_pixel", "GET", "/entries/8/fragment", ALICE),
    ("summary_fragment_completed", "GET", "/entries/1/summary/fragment", ADMIN),
    ("summary_fragment_failed", "GET", "/entries/5/summary/fragment", ADMIN),
    ("summary_fragment_none", "GET", "/entries/2/summary/fragment", ADMIN),
    ("feeds", "GET", "/feeds", ADMIN),
    ("feed_edit_1", "GET", "/feeds/1/edit", ADMIN),
    ("feed_edit_2_custom", "GET", "/feeds/2/edit", ADMIN),
    ("feed_edit_3_error", "GET", "/feeds/3/edit", ADMIN),
    ("feeds_import", "GET", "/feeds/import", ADMIN),
    ("feed_entries", "GET", "/feeds/1/entries", ADMIN),
    ("feed_entries_read", "GET", "/feeds/1/entries?status=read", ADMIN),
    ("feed_entries_starred", "GET", "/feeds/1/entries?status=starred", ADMIN),
    ("categories", "GET", "/categories", ADMIN),
    ("category_entries", "GET", "/categories/1/entries", ADMIN),
    ("category_entries_fragment", "GET", "/categories/1/entries?fragment=1", ADMIN),
    ("search_empty", "GET", "/search", ADMIN),
    ("search_results", "GET", "/search?q=alpha", ADMIN),
    ("search_no_results", "GET", "/search?q=zzzznothing", ADMIN),
    ("search_syntax_error", "GET", "/search?q=%22unterminated", ADMIN),
    ("search_fragment", "GET", "/search?q=alpha&fragment=1", ADMIN),
    ("search_operators", "GET", "/search?q=is%3Astarred+feed%3A1", ADMIN),
    ("statistics", "GET", "/statistics", ADMIN),
    ("summarizer", "GET", "/summarizer", ADMIN),
    ("user_settings_admin", "GET", "/user-settings", ADMIN),
    ("user_settings_alice", "GET", "/user-settings", ALICE),
    ("settings", "GET", "/settings", ADMIN),
    ("admin", "GET", "/admin", ADMIN),
    ("admin_as_user", "GET", "/admin", ALICE),
    ("offline", "GET", "/offline", ADMIN),
    ("not_found", "GET", "/no/such/page", ADMIN),
    ("home_alice", "GET", "/", ALICE),
    ("api_sidebar", "GET", "/api/sidebar", ADMIN),
    ("api_sidebar_category_feeds", "GET", "/api/sidebar/categories/1/feeds", ADMIN),
    ("api_user", "GET", "/api/user", ADMIN),
    ("api_offline_manifest", "GET", "/api/offline/manifest", ALICE),
    ("api_passkeys", "GET", "/api/passkeys", ADMIN),
    ("login_signed_in", "GET", "/login", ADMIN),
]


def main():
    work, out_dir = sys.argv[2], sys.argv[3]
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    shutil.rmtree(out_dir, ignore_errors=True)
    os.makedirs(out_dir)
    db_path = os.path.join(work, "golden.sqlite3")
    log_path = os.path.join(work, "server.log")
    expires = (datetime.now(timezone.utc) + timedelta(days=7)).strftime("%Y-%m-%dT%H:%M:%S.000000+00:00")
    cap = Capture(out_dir, expires)

    proc = start_server(db_path, log_path)
    try:
        cap.take("setup", "GET", "/setup", anonymous=True)
        cap.take("login_setup_available", "GET", "/login", anonymous=True)
        cap.take("setup_bad_token", "POST", "/setup", anonymous=True,
                 body="username=root&password=correct+horse+battery+staple&"
                      "confirm-password=correct+horse+battery+staple&setup-token=AAAAA-AAAAA")
        cap.take("setup_mismatch", "POST", "/setup", anonymous=True,
                 body="username=root&password=correct+horse+battery+staple&"
                      "confirm-password=different&setup-token=AAAAA-AAAAA")
    finally:
        stop_server(proc)

    os.remove(db_path + "-wal") if os.path.exists(db_path + "-wal") else None
    apply_seed(db_path, expires)

    proc = start_server(db_path, log_path)
    try:
        cap.take("login", "GET", "/login", anonymous=True)
        cap.take("login_next", "GET", "/login?next=%2Fentries", anonymous=True)
        cap.take("login_bad_credentials", "POST", "/login", anonymous=True,
                 body="username=admin&password=wrong&next=%2Fentries")
        cap.take("invite", "GET", "/invite/golden-invite-token", anonymous=True)
        cap.take("invite_invalid", "GET", "/invite/not-a-token", anonymous=True)
        cap.take("invite_mismatch", "POST", "/invite/golden-invite-token", anonymous=True,
                 body="password=correct+horse+battery+staple&confirm_password=different")
        for name, method, path, user in AUTHED_PAGES:
            cap.take(name, method, path, user=user)
        # Pages above must not have mutated state that later pages show; reload twice
        # to prove stability of the first listing.
        cap.take("home_again", "GET", "/", user=ADMIN)
    finally:
        stop_server(proc)

    with open(os.path.join(out_dir, "index.json"), "w", encoding="utf-8") as fh:
        json.dump({"version_placeholder": "@VERSION@", "pages": cap.index}, fh, indent=1)
        fh.write("\n")
    print(f"captured {len(cap.index)} responses; version={cap.version!r}")


if __name__ == "__main__":
    main()
