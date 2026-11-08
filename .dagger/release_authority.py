"""Validate the fork's release ownership without executing any publishers."""

RELEASE_WORKFLOWS = {
    'release.yml',
    'release-for-mac.yml',
    'release-for-windows.yml',
    'release-slint-desktop.yml',
}
PUBLISH_RELEASE = (
    "${{ github.event_name == 'release' || "
    "(github.event_name == 'workflow_dispatch' && inputs.publish) }}"
)


def check_release_authority(workflows):
    assert RELEASE_WORKFLOWS <= workflows.keys(), 'Missing fork release workflow'
    for name, doc in workflows.items():
        # PyYAML's YAML 1.1 loader interprets the Actions key "on" as True.
        events = doc.get('on', doc.get(True, {}))
        if 'release' in events:
            assert name in RELEASE_WORKFLOWS, (name, 'Duplicate release publisher')
        for job in doc['jobs'].values():
            for step in job.get('steps', []):
                script = step.get('run', '')
                assert '.dagger/modules/github-assets' not in script, name
                assert '.dagger/modules/gemfury' not in script, name
                assert '--clobber' not in script, name

    for name in RELEASE_WORKFLOWS:
        doc = workflows[name]
        events = doc.get('on', doc.get(True, {}))
        assert events['workflow_dispatch']['inputs']['publish']['default'] is False, name
        if name == 'release.yml':
            build = doc['jobs']['build']
            targets = {row['target'] for row in build['strategy']['matrix']['include']}
            assert targets == {'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu'}
            prepare = doc['jobs']['prepare']['steps']
            metadata = [s for s in prepare if s.get('run') == 'python3 scripts/release-metadata.py']
            assert len(metadata) == 1
            assert metadata[0]['env']['PUBLISH'] == "${{ github.event_name == 'release' || inputs.publish == true }}"
            publish = doc['jobs']['publish']
            assert publish['if'] == "needs.prepare.outputs.publish == 'true'"
            assert set(publish['needs']) == {'prepare', 'build'}
            assert sum(s.get('run') == 'python3 scripts/publish-headless.py' for s in publish['steps']) == 1
            continue

        assert doc['env']['PUBLISH_RELEASE'] == PUBLISH_RELEASE, name
        steps = [s for job in doc['jobs'].values() for s in job.get('steps', [])]
        publishers = [s for s in steps if 'scripts/release-assets.py publish ' in s.get('run', '')]
        assert len(publishers) == (3 if name == 'release-slint-desktop.yml' else 1), name
        guards = {
            "env.PUBLISH_RELEASE == 'true'",
            "env.PUBLISH_RELEASE == 'true' && runner.os == 'macOS'",
            "env.PUBLISH_RELEASE == 'true' && runner.os == 'Linux'",
        }
        assert all(s.get('if') in guards for s in publishers), name
        for step in steps:
            if step.get('id') == 'gemfury':
                assert step['if'] == "github.event_name == 'release' && runner.os == 'Linux'", name
            if 'push.fury.io' in step.get('run', ''):
                assert step['if'] == "runner.os == 'Linux' && steps.gemfury.outputs.available == 'true'", name
