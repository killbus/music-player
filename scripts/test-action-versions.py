"""Exercise release selection and public API transport without network access."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('audit', Path(__file__).with_name('check-action-versions.py'))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class ActionVersionTests(unittest.TestCase):
    def test_regular_action_uses_latest(self):
        release = {'tag_name': 'v7.0.1'}
        read = Mock(return_value=release)
        self.assertEqual(audit.latest_action_release('actions/checkout', read), release)
        read.assert_called_once_with('repos/actions/checkout/releases/latest')

    def test_codeql_ignores_bundles_prereleases_and_older_major(self):
        def release(tag, **kwargs):
            return dict(tag_name=tag, draft=False, prerelease=False, **kwargs)

        rows = [release('v3.99.0'), release('codeql-bundle-v2.27.4'),
                release('v4.9.3'), release('v4.10.0'),
                dict(tag_name='v5.0.0', draft=False, prerelease=True),
                dict(tag_name='v6.0.0', draft=True, prerelease=False)]
        read = Mock(side_effect=[rows[1], rows])
        self.assertEqual(audit.latest_action_release('github/codeql-action', read)['tag_name'], 'v4.10.0')

    @patch.object(audit.subprocess, 'check_output')
    def test_curl_uses_public_api_and_fails_on_http_errors(self, output):
        output.return_value = json.dumps({'tag_name': 'v1'})
        self.assertEqual(audit.api('repos/owner/repo/releases/latest', 'curl'), {'tag_name': 'v1'})
        command = output.call_args.args[0]
        self.assertIn('--fail-with-body', command)
        self.assertEqual(command[-1], 'https://api.github.com/repos/owner/repo/releases/latest')

    @patch.object(audit.subprocess, 'check_output')
    def test_ci_keeps_gh_as_default(self, output):
        output.return_value = '{}'
        audit.api('repos/owner/repo/releases/latest')
        self.assertEqual(output.call_args.args[0], ['gh', 'api', 'repos/owner/repo/releases/latest'])


if __name__ == '__main__':
    unittest.main()
