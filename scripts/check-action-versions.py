#!/usr/bin/env python3
"""Re-fetch release and docs for the Actions in the specified workflows."""
import argparse
import base64
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], encoding='utf-8', timeout=60))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('workflows', nargs='*', default=['.github/workflows/release.yml'])
    args = parser.parse_args()
    records = {row['repository'].removeprefix('https://github.com/'): row
               for row in json.loads((ROOT / '.github/action-versions.json').read_text())}
    checked = set()
    for workflow in args.workflows:
        for ref in re.findall(r'(?m)^\s*-?\s*uses:\s*[\"\x27]?([^\s\"\x27#]+)',
                              (ROOT / workflow).read_text()):
            if ref.startswith('./'):
                continue
            action, pin = ref.rsplit('@', 1)
            repo = '/'.join(action.split('/')[:2])
            row = records[repo]
            assert re.fullmatch(r'[0-9a-f]{40}', pin), f'{ref}: full SHA required'
            assert pin == row['sha'], f'{ref}: pin differs from audit'
            if repo in checked:
                continue
            release = api(f'repos/{repo}/releases/latest')
            assert release['tag_name'] == row['tag'], f'{repo}: latest is {release["tag_name"]}'
            assert api(f'repos/{repo}/commits/{row["tag"]}')["sha"] == pin
            for doc in ('README.md', 'action.yml'):
                result = api(f'repos/{repo}/contents/{doc}?ref={row["tag"]}')
                assert base64.b64decode(result['content']).strip(), f'{repo}: empty {doc}'
            checked.add(repo)
            print(f'{row["repository"]}: {row["tag"]} {pin}; release and docs fetched')


if __name__ == '__main__':
    main()
