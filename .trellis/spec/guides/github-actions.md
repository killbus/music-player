# GitHub Actions: verify current upstream facts

For every external Action in a new or modified workflow:

1. Parse owner/repository[/subpath] from uses and construct
   https://github.com/owner/repository. Do not select a version from memory.
2. Fetch https://api.github.com/repos/owner/repository/releases/latest at the
   time of the change. Read release notes, README and action.yml/action.yaml at
   that release, including runner requirements, inputs and migration notes.
3. Use the latest stable release, resolving its tag to a full commit SHA. Pin
   that SHA in uses with a release-version comment. Never execute a floating
   latest/main reference or resolve executable Actions dynamically during CI.
4. Record repository URL, release URL/tag, SHA, UTC check date, documentation
   links and relevant facts in .github/action-versions.json and task research.
5. Re-fetch before completing the workflow change. A previous audit is evidence
   for that date, not proof that its version is still latest. If a release is
   incompatible, document the concrete incompatibility; do not silently use an
   old major. If there are no releases, explicitly record that API result and
   the upstream documented versioning convention before selecting a commit.
6. Local ./ Actions have no separate release: inspect their action metadata.
   For docker:// references, verify the image publisher/tag/digest instead.
   Review nested external Actions in a composite Action when adopting it.

Use scripts/check-action-versions.py <workflow...> to recheck the recorded
releases, pins and documentation availability. Read fetched documentation;
a passing version comparison alone is not a semantic review. Legacy workflows
are audited when touched, rather than being silently represented as audited.
