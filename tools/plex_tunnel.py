"""Open a Cloudflare quick tunnel to open-plex, and publish its address to plex.tv.

A quick tunnel gets a new `https://*.trycloudflare.com` address every time it starts. This
starts `cloudflared`, waits for the address, and hands it to the running server through the
`moveExtension` mutation, which stores it and publishes it to plex.tv at once — no restart,
no pairing again. Then it stays in the foreground with the tunnel; Ctrl-C closes both.

The origin must not be a loopback address. A trusted network is admitted without a token,
and `127.0.0.1` is the one usually trusted: a tunnel ending there would hand every stranger
the operator's rights. Point it at the machine's LAN address, which the server must listen
on (`OPENPLEX_BIND=0.0.0.0:32500`).

Usage:
    python tools/plex_tunnel.py --origin http://192.168.1.30:32500 [--email a@example.org]
        [--cloudflared PATH] [--graphql http://127.0.0.1:32500/graphql]

The mutation needs the operator: run it from a trusted network, or set OPENPLEX_TOKEN.
"""

from __future__ import annotations

import argparse
import ipaddress
import json
import os
import re
import subprocess
import sys
import urllib.parse
import urllib.request

ADDRESS = re.compile(r"https://[a-z0-9-]+\.trycloudflare\.com")

LINKED = """{ users(first: 500) { edges { node { id email
  extensions { edges { node { extension account { connectionUri } } } } } } } }"""
MOVE = """mutation($input: MoveExtensionInput!) {
  moveExtension(input: $input) { connectionUri publishedAt } }"""


def graphql(endpoint: str, query: str, variables: dict | None = None) -> dict:
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    token = os.environ.get("OPENPLEX_TOKEN")
    if token:
        headers["X-Plex-Token"] = token
    body = json.dumps({"query": query, "variables": variables or {}}).encode()
    request = urllib.request.Request(endpoint, data=body, headers=headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        answer = json.load(response)
    if answer.get("errors"):
        raise SystemExit(f"the server refused: {answer['errors'][0]['message']}")
    return answer["data"]


def paired_user(endpoint: str, email: str | None) -> tuple[str, str]:
    """The user whose Plex connection is paired: the one named, or the only one."""
    paired = []
    for edge in graphql(endpoint, LINKED)["users"]["edges"]:
        user = edge["node"]
        for extension in user["extensions"]["edges"]:
            if extension["node"]["extension"] == "PLEX" and extension["node"]["account"]:
                paired.append((user["id"], user["email"]))
    if email:
        paired = [user for user in paired if user[1] == email]
    if len(paired) != 1:
        names = ", ".join(user[1] for user in paired) or "none"
        raise SystemExit(f"expected one paired user, found {names}: name one with --email")
    return paired[0]


def refuse_loopback(origin: str) -> None:
    host = urllib.parse.urlsplit(origin).hostname or ""
    try:
        loopback = ipaddress.ip_address(host).is_loopback
    except ValueError:
        loopback = host == "localhost"
    if loopback:
        raise SystemExit(
            f"{origin} is a loopback address, which open-plex usually admits without a token: "
            "point the tunnel at the machine's LAN address instead"
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--origin", required=True, help="where cloudflared forwards, http://LAN-IP:port")
    parser.add_argument("--email", help="the paired user to move, when there are several")
    parser.add_argument("--cloudflared", default="cloudflared")
    parser.add_argument("--graphql", default="http://127.0.0.1:32500/graphql")
    args = parser.parse_args()
    refuse_loopback(args.origin)
    user, email = paired_user(args.graphql, args.email)

    tunnel = subprocess.Popen(
        [args.cloudflared, "tunnel", "--no-autoupdate", "--url", args.origin],
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    try:
        published = False
        assert tunnel.stderr is not None
        for line in tunnel.stderr:
            sys.stderr.write(line)
            found = ADDRESS.search(line)
            if found and not published:
                moved = graphql(args.graphql, MOVE, {"input": {
                    "userId": user, "extension": "PLEX", "connectionUri": found.group(0),
                }})["moveExtension"]
                state = "published" if moved["publishedAt"] else "stored; plex.tv did not answer, the server retries hourly"
                print(f"\n{email}: {moved['connectionUri']} ({state})\n", flush=True)
                published = True
    except KeyboardInterrupt:
        pass
    finally:
        tunnel.terminate()


if __name__ == "__main__":
    main()
