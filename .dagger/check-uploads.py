"""Check fork release guards and execute Codecov blocks with a fake Dagger."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

import yaml
from release_authority import check_release_authority

def check_codecov(repo):
    for name, report, flags in [('ci.yml', 'artifacts/lcov.info', []), ('webui-tests.yml', 'artifacts/webui/coverage/lcov.info', ['--flags', 'webui'])]:
        doc = yaml.safe_load((repo / '.github/workflows' / name).read_text())
        steps = [step for job in doc['jobs'].values() for step in job.get('steps', [])]
        assert not any('codecov/codecov-action' in step.get('uses', '') for step in steps)
        uploads = [step for step in steps if '.dagger/modules/codecov upload' in step.get('run', '')]
        assert len(uploads) == 1
        step = uploads[0]
        assert step['continue-on-error'] is True
        assert 'pull_request.head.sha' in step['env']['COVERAGE_COMMIT']
        assert 'github.head_ref' in step['env']['COVERAGE_BRANCH']
        assert 'pull_request.number' in step['env']['COVERAGE_PR']
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            fake = cwd / 'dagger'
            fake.write_text('#!/usr/bin/env python3\nimport json,os,sys\nopen(os.environ["CALL_LOG"],"w").write(json.dumps(sys.argv[1:]))\n')
            fake.chmod(0o755)
            for pr in ['', '42']:
                env = dict(os.environ, PATH=str(cwd)+os.pathsep+os.environ['PATH'], CALL_LOG=str(cwd/'calls'), GITHUB_REPOSITORY='owner/repo', GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='2', GITHUB_JOB='tests', CODECOV_TOKEN='unused', COVERAGE_COMMIT='a'*40, COVERAGE_BRANCH='feature/with spaces', COVERAGE_PR=pr)
                subprocess.run(['bash', '-euo', 'pipefail', '-c', step['run']], cwd=cwd, env=env, check=True)
                args = json.loads((cwd/'calls').read_text())
                assert args == ['--progress', 'logs', '-v', 'api', 'call', '-m', '.dagger/modules/codecov', 'upload', '--report', report, '--repository', 'owner/repo', '--commit', 'a'*40, '--branch', 'feature/with spaces', '--pull-request', pr, '--token', 'env:CODECOV_TOKEN', '--run-id', '123-2-tests'] + flags, args
    print('Checked both Codecov workflow calls for push and PR metadata with a fake executable.')

if __name__ == '__main__':
    root=Path(__file__).resolve().parent.parent
    workflows = {
        path.name: yaml.safe_load(path.read_text(encoding='utf-8'))
        for path in (root / '.github/workflows').glob('*.yml')
    }
    check_release_authority(workflows)
    subprocess.run([sys.executable, str(root / '.dagger/test_release_authority.py')], check=True)
    print('Checked fork release ownership, publish gates and both Linux architectures.')
    subprocess.run([sys.executable, str(root / 'scripts/test-action-versions.py')], check=True)
    check_codecov(root)
