"""Sign in to a running klapz as its built-in `admin` account, for the other tools.

Every request to klapz comes from a user. A tool signs in through the `signIn` mutation
with the password in `KLAPZ_ADMIN_PASSWORD`, reads the session the server sets in its
`klapz_session` cookie, and sends it back as `Authorization: Bearer <session>` — on
`/graphql` and on the Plex surface alike. Neither the password nor the session is ever
printed: the session opens everything the admin can.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from http.cookies import SimpleCookie

ADMIN_USERNAME = "admin"
SESSION_COOKIE = "klapz_session"

SIGN_IN = """mutation($input: SignInInput!) {
  signIn(input: $input) { user { username } userErrors { code message } } }"""


def sign_in(graphql_endpoint: str) -> str:
    """The admin's session, to present as a bearer token."""
    password = os.environ.get("KLAPZ_ADMIN_PASSWORD")
    if not password:
        raise SystemExit("set KLAPZ_ADMIN_PASSWORD to the admin account's password")
    body = json.dumps({
        "query": SIGN_IN,
        "variables": {"input": {"login": ADMIN_USERNAME, "password": password}},
    }).encode()
    request = urllib.request.Request(
        graphql_endpoint,
        data=body,
        headers={"Content-Type": "application/json", "Accept": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            answer = json.load(response)
            cookies = response.headers.get_all("Set-Cookie") or []
    except urllib.error.HTTPError as error:
        raise SystemExit(f"signing in failed: HTTP {error.code}") from None
    if answer.get("errors"):
        raise SystemExit(f"signing in failed: {answer['errors'][0]['message']}")
    refused = answer["data"]["signIn"]["userErrors"]
    if refused:
        raise SystemExit(f"signing in as {ADMIN_USERNAME} was refused: {refused[0]['message']}")
    for header in cookies:
        jar = SimpleCookie()
        jar.load(header)
        if SESSION_COOKIE in jar and jar[SESSION_COOKIE].value:
            return jar[SESSION_COOKIE].value
    raise SystemExit("the server signed in without setting a session cookie")


def authorization(session: str) -> dict[str, str]:
    """The header that presents a session."""
    return {"Authorization": f"Bearer {session}"}
