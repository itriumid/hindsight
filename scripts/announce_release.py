#!/usr/bin/env python3
"""Posts a published release to Itrium's Discord server, in #releases.

    python3 scripts/announce_release.py <release.json> [--dry-run]

<release.json> is a GitHub release object: the event payload's `release` in a workflow, or
`gh api repos/itriumid/hindsight/releases/tags/<tag>` by hand. The webhook address comes from
the DISCORD_RELEASES_WEBHOOK environment variable and is never printed. With --dry-run, the
message is printed instead of sent. Standard library only.

It notifies nobody: there's no role for Hindsight to ping, and the post is news, not a call to test.
"""
import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

NAME = "Hindsight"
PAGE = "itrium.id/hindsight"
PINK = 0xFEBFCA  # Rhodonite's pastel pink, the brand's accent.
DESCRIPTION_LIMIT = 4096  # Discord's limit for an embed's description.
ICON = "https://raw.githubusercontent.com/itriumid/hindsight/main/branding/icon.png"
# (what people call it, a pattern matching the release file's whole name)
DOWNLOADS = [
    ("Mac", r"Hindsight_universal\.dmg"),
    ("Windows", r"Hindsight_x64-setup\.exe"),
    ("Linux", r"Hindsight_amd64\.deb"),
]
# Links that aren't release files, shown after the downloads.
ALSO = [
    ("Everything else", "https://itrium.id/hindsight#download"),
]


def notes(body, url):
    """The release notes in Discord's markdown: headings become bold lines (embeds don't
    show headings), and GitHub's own footer lines go."""
    lines = []
    skipping = False
    for line in (body or "").replace("\r\n", "\n").split("\n"):
        heading = re.match(r"^#{1,6}\s+(.*)$", line)
        if heading:
            title = heading.group(1).strip()
            skipping = title.lower() == "new contributors"
            if skipping or title.lower() == "what's new" or title.lower() == "what's changed":
                continue  # The embed's title already says it.
            line = f"**{title}**"
        if skipping or line.startswith("**Full Changelog**"):
            continue
        # GitHub's generated notes credit an author on every line; the pull request is enough.
        line = re.sub(r" by @[\w-]+ in https://github\.com/[^/\s]+/[^/\s]+/pull/(\d+)", r" (#\1)", line)
        lines.append(line)
    text = re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).strip()
    if len(text) <= DESCRIPTION_LIMIT:
        return text
    more = f"\n\n[Read the rest on GitHub]({url})"
    cut = text[: DESCRIPTION_LIMIT - len(more)]
    # End at a whole line, so a bullet or a link is never cut in half.
    return cut[: cut.rfind("\n")].rstrip() + more


def links(assets):
    found = []
    for label, pattern in DOWNLOADS:
        asset = next((a for a in assets if re.fullmatch(pattern, a["name"])), None)
        if asset:
            found.append(f"[{label}]({asset['browser_download_url']})")
    found += [f"[{label}]({url})" for label, url in ALSO]
    return " · ".join(found)


def message(release):
    embed = {
        "title": release["name"] or f"{NAME} {release['tag_name']}",
        "url": release["html_url"],
        "color": PINK,
        "description": notes(release.get("body"), release["html_url"]),
        "footer": {"text": PAGE},
    }
    if release.get("published_at"):
        embed["timestamp"] = release["published_at"]
    found = links(release.get("assets", []))
    if found:
        embed["fields"] = [{"name": "Download", "value": found}]
    return {
        "username": NAME,
        "avatar_url": ICON,
        "embeds": [embed],
        # Nobody is notified: never @everyone or a role by accident.
        "allowed_mentions": {"parse": []},
    }


def send(payload, webhook):
    request = urllib.request.Request(webhook + "?wait=true", data=json.dumps(payload).encode(), method="POST",
                                     headers={"Content-Type": "application/json", "User-Agent": "hindsight"})
    try:
        with urllib.request.urlopen(request, timeout=30):
            pass
    except urllib.error.HTTPError as error:
        # The error body is Discord's explanation; the address itself is never shown.
        sys.exit(f"Discord refused the announcement ({error.code}): {error.read().decode(errors='replace')[:500]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("release", help="a GitHub release object, as JSON")
    parser.add_argument("--dry-run", action="store_true", help="print the message instead of sending it")
    arguments = parser.parse_args()
    with open(arguments.release) as file:
        release = json.load(file)
    if release.get("draft"):
        sys.exit("That release is still a draft. Publish it first.")
    payload = message(release)
    if arguments.dry_run:
        print(json.dumps(payload, indent=2, ensure_ascii=False))
        return
    webhook = os.environ.get("DISCORD_RELEASES_WEBHOOK")
    if not webhook:
        sys.exit("DISCORD_RELEASES_WEBHOOK isn't set. See the comment at the top of the workflow.")
    send(payload, webhook)
    print(f"Announced {payload['embeds'][0]['title']} on Discord.")


if __name__ == "__main__":
    main()
