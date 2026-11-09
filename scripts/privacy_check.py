"""Check tracked files without printing sensitive matches. No third-party service."""
import argparse
import pathlib
import re
import subprocess
import sys

RULES = {
    "access token": re.compile(r"(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{30,}|AKIA[A-Z0-9]{16})"),
    "private key": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "personal home path": re.compile(r"(?i)(?:[a-z]:[\\/]Users[\\/][^\\/\s]+|/(?:Users|home)/[^/\s]+)"),
    "private IPv4": re.compile(r"(?<![\d.])(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})(?![\d.])"),
    "email address": re.compile(r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}"),
}
DENIED_SUFFIXES = {".pem", ".key", ".p12", ".pfx", ".log"}

def scan(text):
    return [name for name, pattern in RULES.items() if pattern.search(text)]

def self_test():
    assert "access token" in scan("gh" + "p_" + "x" * 36)
    assert "personal home path" in scan("C:" + chr(92) + "Users" + chr(92) + "example" + chr(92) + "file")
    assert "private IPv4" in scan("192" + ".168.99.7")
    assert not scan("127.0.0.1 <HOST_IP> contributors")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--binary", type=pathlib.Path)
    args = parser.parse_args()
    if args.self_test:
        self_test()
    if args.binary:
        data = args.binary.read_bytes()
        text = data.decode("utf-8", errors="ignore") + data.decode("utf-16-le", errors="ignore")
        # Binary codec/library constants can resemble tokens/IPs. Source scanning
        # remains exhaustive; release binaries are checked for personal paths.
        failures = ["personal home path"] if RULES["personal home path"].search(text) else []
        if failures:
            print("FAIL: binary contains a personal home path (match redacted)")
            return 1
        print("PASS: release binary path check")
        return 0
    files = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0")
    failures = []
    for name in filter(None, files):
        path = pathlib.Path(name)
        if path.suffix.lower() in DENIED_SUFFIXES or path.name.startswith(".env") or path.name == "MAC-DISPLAY-RESEARCH.md":
            failures.append((name, "private/runtime file"))
            continue
        if not path.is_file():
            continue
        for category in scan(path.read_text(encoding="utf-8", errors="ignore")):
            failures.append((name, category))
    for name, category in failures:
        print(f"FAIL: {name}: {category} (match redacted)")
    if failures:
        return 1
    print(f"PASS: {sum(bool(f) for f in files)} tracked files checked; no sensitive matches")
    return 0

if __name__ == "__main__":
    sys.exit(main())
