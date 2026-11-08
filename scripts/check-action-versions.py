#!/usr/bin/env python3
"""Re-fetch release and docs for the Actions in the specified workflows."""
import argparse
import base64
import json
import re
import subprocess
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def api(path, client='gh'):
    command = ['gh', 'api', path]
    if client == 'curl':
        command = ['curl', '--fail-with-body', '-sS', '-H',
                   'Accept: application/vnd.github+json', f'https://api.github.com/{path}']
    return json.loads(subprocess.check_output(command, encoding='utf-8', timeout=60))


def latest_action_release(repo, read_api):
    release = read_api(f'repos/{repo}/releases/latest')
    if repo != 'github/codeql-action':
        return release
    # CodeQL's latest endpoint points at a CLI bundle, not an Action release.
    releases = read_api(f'repos/{repo}/releases?per_page=100')
    actions = [row for row in releases if not row['draft'] and not row['prerelease']
               and re.fullmatch(r'v\d+\.\d+\.\d+', row['tag_name'])]
    return max(actions, key=lambda row: tuple(map(int, row['tag_name'][1:].split('.'))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--api-client', choices=['gh', 'curl'], default='gh',
                        help='curl reads public repositories without gh authentication (60 requests/hour)')
    parser.add_argument('workflows', nargs='*',
                        default=['.github/workflows/release.yml',
                                 '.github/workflows/release-for-mac.yml',
                                 '.github/workflows/release-slint-desktop.yml',
                                 '.github/workflows/release-for-windows.yml'])
    args = parser.parse_args()
    read_api = partial(api, client=args.api_client)
    records = {row['repository'].removeprefix('https://github.com/'): row
               for row in json.loads((ROOT / '.github/action-versions.json').read_text(encoding='utf-8'))}
    checked = set()
    for workflow in args.workflows:
        for ref in re.findall(r'(?m)^\s*-?\s*uses:\s*[\"\x27]?([^\s\"\x27#]+)',
                              (ROOT / workflow).read_text(encoding='utf-8')):
            if ref.startswith('./'):
                continue
            action, pin = ref.rsplit('@', 1)
            repo = '/'.join(action.split('/')[:2])
            row = records[repo]
            assert re.fullmatch(r'[0-9a-f]{40}', pin), f'{ref}: full SHA required'
            assert pin == row['sha'], f'{ref}: pin differs from audit'
            if repo in checked:
                continue
            release = latest_action_release(repo, read_api)
            assert release['tag_name'] == row['tag'], f'{repo}: latest is {release["tag_name"]}'
            assert read_api(f'repos/{repo}/commits/{row["tag"]}')["sha"] == pin
            metadata = row['metadata'].split(f'/blob/{row["tag"]}/', 1)[1]
            for doc in ('README.md', metadata):
                result = read_api(f'repos/{repo}/contents/{doc}?ref={row["tag"]}')
                assert base64.b64decode(result['content']).strip(), f'{repo}: empty {doc}'
            checked.add(repo)
            print(f'{row["repository"]}: {row["tag"]} {pin}; release and docs fetched')


if __name__ == '__main__':
    main()
