"""Negative controls for the release workflow migration guard."""
from copy import deepcopy
from pathlib import Path
import unittest

import yaml

from release_authority import check_release_authority


class ReleaseAuthorityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        root = Path(__file__).resolve().parent.parent
        cls.workflows = {
            p.name: yaml.safe_load(p.read_text(encoding='utf-8'))
            for p in (root / '.github/workflows').glob('*.yml')
        }

    def setUp(self):
        self.docs = deepcopy(self.workflows)

    def test_current_workflows(self):
        check_release_authority(self.docs)

    def test_duplicate_release_workflow_is_rejected(self):
        self.docs['release-for-arm.yml'] = deepcopy(self.docs['release.yml'])
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)

    def test_dagger_asset_publisher_is_rejected_even_on_dispatch(self):
        self.docs['unexpected.yml'] = {
            'on': {'workflow_dispatch': None},
            'jobs': {'release': {'steps': [{'run': 'dagger api call -m .dagger/modules/github-assets upload'}]}},
        }
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)

    def test_headless_publish_requires_build(self):
        self.docs['release.yml']['jobs']['publish']['needs'] = ['prepare']
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)

    def test_headless_publish_cannot_be_unconditional(self):
        del self.docs['release.yml']['jobs']['publish']['if']
        with self.assertRaises((AssertionError, KeyError)):
            check_release_authority(self.docs)

    def test_ordinary_dispatch_cannot_publish(self):
        doc = self.docs['release.yml']
        events = doc.get('on', doc.get(True))
        events['workflow_dispatch']['inputs']['publish']['default'] = True
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)

    def test_desktop_upload_requires_publish_gate(self):
        steps = self.docs['release-slint-desktop.yml']['jobs']['release']['steps']
        step = next(s for s in steps if 'scripts/release-assets.py publish ' in s.get('run', ''))
        del step['if']
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)

    def test_arm64_must_remain_in_canonical_pipeline(self):
        matrix = self.docs['release.yml']['jobs']['build']['strategy']['matrix']
        matrix['include'] = [row for row in matrix['include'] if row['arch'] != 'arm64']
        with self.assertRaises(AssertionError):
            check_release_authority(self.docs)


if __name__ == '__main__':
    unittest.main()
