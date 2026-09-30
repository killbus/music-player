#!/usr/bin/env python3
"""Run the real postinst with fake accounts/systemd; no root or Linux host needed.

Usage: python3 scripts/test-service-install.py
Requires Bash (Git Bash on Windows); set SERVICE_TEST_BASH to its executable if
not on PATH. This checks maintscript control flow, not real systemd/NSS behavior
or migration of private state. No host accounts, services or policy are changed.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve()
POSTINST = SCRIPT.parents[1] / "dist/debian/postinst"
UNIT = "music-player.service"

# Source the actual maintscript, unchanged. Only host probes and external
# commands are replaced. Bash permits a function for the absolute policy path.
HARNESS = r'''
set -eu
mock() { "$SERVICE_TEST_PYTHON" "$SERVICE_TEST_SCRIPT" --mock "$@"; }
function [() {
    if builtin [ "$#" -eq 3 ]; then
        case "$1:$2" in
            -d:/run/systemd/system) mock probe systemd; return $? ;;
            -x:/usr/sbin/policy-rc.d) return 0 ;;
        esac
    fi
    builtin [ "$@"
}
systemctl() { mock systemctl "$@"; }
getent() { mock getent "$@"; }
adduser() { mock adduser "$@"; }
addgroup() { mock addgroup "$@"; }
deb-systemd-helper() { mock deb-systemd-helper "$@"; }
deb-systemd-invoke() { mock deb-systemd-invoke "$@"; }
policy-rc.d() { mock policy-rc.d "$@"; }
function /usr/sbin/policy-rc.d() { mock policy-rc.d "$@"; }
set -- configure
. "$SERVICE_TEST_POSTINST"
'''


def mock_command(state, command, args):
    """Model command contracts, including successful policy/disabled no-ops."""
    state["calls"].append([command, *args])
    if command == "probe" and args == ["systemd"]:
        return 0, ""
    if command == "getent" and args in (["passwd", "music-player"],
                                          ["group", "music-player"]):
        transient = state["active"] and state["dynamic"]
        if args[0] == "passwd":
            if state["persistent"]:
                return 0, "music-player:x:123:123::/var/lib/music-player:/usr/sbin/nologin"
            if transient:
                return 0, "music-player:x:61234:61234:Dynamic User:/:/usr/sbin/nologin"
        elif state["group"] or transient:
            return 0, "music-player:x:123:"
        return 2, ""
    if command == "adduser":
        expected = ["--system", "--group", "--home", "/var/lib/music-player",
                    "--no-create-home", "--shell", "/usr/sbin/nologin", "music-player"]
        if args != expected:
            raise ValueError("unexpected account creation arguments")
        if state["persistent"] or (state["active"] and state["dynamic"]):
            return 1, "account already exists"
        state["persistent"] = state["group"] = True
        state["effects"].append("create-account")
        return 0, ""
    if command == "addgroup" and args == ["--system", "music-player"]:
        state["group"] = True
        return 0, ""
    if command == "deb-systemd-helper" and args == ["update-state", UNIT]:
        return 0, ""
    if command == "policy-rc.d":
        if UNIT not in args:
            raise ValueError("policy lookup missing unit")
        action = next((a for a in args if a in ("start", "stop", "restart")), None)
        if action is None:
            raise ValueError("policy lookup missing action")
        state["policy_checks"].append(action)
        return (101 if action in state["deny"] else 0), ""
    if command == "deb-systemd-invoke" and len(args) == 2 and args[1] == UNIT:
        action = args[0]
        if action not in ("stop", "start", "restart"):
            raise ValueError("unsupported helper action")
        state["policy_checks"].append(action)
        if action in state["deny"]:
            return 0, "policy-rc.d returned 101"
        # Debian's helper skips disabled starts, and disabled inactive restarts.
        if not state["enabled"] and (action == "start" or
                (action == "restart" and not state["active"])):
            return 0, "disabled unit, not starting"
        return service_action(state, action)
    if command == "systemctl":
        args = [arg for arg in args if arg not in ("--", "--quiet", "--system")]
        if args == ["is-active", UNIT]:
            return (0 if state["active"] else 3), ""
        if args == ["is-enabled", UNIT]:
            return (0 if state["enabled"] else 1), ("enabled" if state["enabled"] else "disabled")
        if args == ["show", "-p", "DynamicUser", "--value", UNIT]:
            return 0, "yes" if state["dynamic"] else "no"
        if args == ["daemon-reload"]:
            state["dynamic"] = False
            state["effects"].append("reload")
            return 0, ""
        if len(args) == 2 and args[1] == UNIT and args[0] in ("start", "stop", "restart"):
            return service_action(state, args[0])
    raise ValueError("unsupported mock command: " + repr([command, *args]))


def service_action(state, action):
    if action == "stop" and state["fail_stop"]:
        return 1, "simulated stop failure"
    if action != "stop" and not state["dynamic"] and not state["persistent"]:
        return 1, "persistent account missing"
    state["active"] = action != "stop"
    state["effects"].append(action)
    return 0, ""


def run_mock():
    # Shell command substitution must receive LF, including from Windows Python.
    sys.stdout.reconfigure(newline="\n")
    path = Path(os.environ["SERVICE_TEST_STATE"])
    state = json.loads(path.read_text(encoding="utf-8"))
    try:
        code, output = mock_command(state, sys.argv[2], sys.argv[3:])
    except ValueError as error:
        state["unexpected"].append(str(error))
        code, output = 99, str(error)
    path.write_text(json.dumps(state), encoding="utf-8")
    if output:
        print(output)
    return code


def find_bash():
    explicit = os.environ.get("SERVICE_TEST_BASH")
    if explicit:
        return explicit
    if os.name != "nt":
        return shutil.which("bash")
    # Avoid the Windows WSL bash launcher; use Git's Bash beside git.exe.
    git = shutil.which("git")
    candidates = []
    if git:
        candidates.extend((Path(git).resolve().parents[1] / "bin/bash.exe",
                           Path(git).resolve().parent / "bash.exe"))
    for variable in ("ProgramFiles", "LOCALAPPDATA"):
        if os.environ.get(variable):
            candidates.append(Path(os.environ[variable]) / "Git/bin/bash.exe")
    return next((str(path) for path in candidates if path.is_file()), None)


class ServiceInstallTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.bash = find_bash()
        if not cls.bash:
            raise RuntimeError("Bash is required; set SERVICE_TEST_BASH to Git Bash on Windows")

    def install(self, **overrides):
        state = dict(active=True, dynamic=True, persistent=False, group=False,
                     enabled=True, deny=[], fail_stop=False, calls=[], effects=[],
                     policy_checks=[], unexpected=[])
        state.update(overrides)
        with tempfile.TemporaryDirectory(prefix="service-install-") as directory:
            state_path = Path(directory) / "state.json"
            state_path.write_text(json.dumps(state), encoding="utf-8")
            environment = os.environ.copy()
            environment.pop("BASH_ENV", None)
            environment.update({
                "SERVICE_TEST_PYTHON": Path(sys.executable).as_posix(),
                "SERVICE_TEST_SCRIPT": SCRIPT.as_posix(),
                "SERVICE_TEST_POSTINST": POSTINST.as_posix(),
                "SERVICE_TEST_STATE": str(state_path),
                "MSYS2_ARG_CONV_EXCL": "*",
                "PYTHONDONTWRITEBYTECODE": "1",
            })
            result = subprocess.run([self.bash, "--noprofile", "--norc", "-c", HARNESS],
                                    env=environment, cwd=directory, capture_output=True,
                                    text=True, timeout=60)
            state = json.loads(state_path.read_text(encoding="utf-8"))
        self.assertEqual(state["unexpected"], [], result.stdout + result.stderr)
        return result, state

    def assert_handover(self, enabled):
        result, state = self.install(enabled=enabled)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(state["persistent"], "persistent account was not created")
        self.assertTrue(state["group"])
        self.assertFalse(state["dynamic"])
        self.assertEqual(state["enabled"], enabled, "upgrade changed enablement")
        self.assertTrue(state["active"], "previously active service was not restored: " + result.stdout)
        self.assertEqual(state["effects"][:3], ["stop", "create-account", "reload"])
        self.assertIn(state["effects"][3:], (["start"], ["restart"]))
        self.assertEqual(state["policy_checks"][0], "stop")
        self.assertIn(state["policy_checks"][-1], ("start", "restart"))

    def test_active_enabled_dynamic_user_handover(self):
        self.assert_handover(enabled=True)

    def test_active_disabled_dynamic_user_handover(self):
        self.assert_handover(enabled=False)

    def test_policy_denies_stop_preserves_dynamic_account(self):
        result, state = self.install(deny=["stop"])
        self.assertNotEqual(result.returncode, 0, "cannot configure while transient identity is live")
        self.assertTrue(state["active"])
        self.assertTrue(state["dynamic"])
        self.assertFalse(state["persistent"])
        self.assertEqual(state["effects"], [])
        self.assertEqual(state["policy_checks"], ["stop"])
        self.assertFalse(any(call[0] in ("adduser", "addgroup") for call in state["calls"]))

    def test_policy_denies_start_after_handover(self):
        result, state = self.install(deny=["start", "restart"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(state["persistent"])
        self.assertFalse(state["active"], "start bypassed policy-rc.d denial")
        self.assertEqual(state["effects"], ["stop", "create-account", "reload"])
        self.assertIn(state["policy_checks"][-1], ("start", "restart"))

    def test_failed_stop_does_not_create_account(self):
        result, state = self.install(fail_stop=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(state["active"])
        self.assertFalse(state["persistent"])
        self.assertEqual(state["effects"], [])

    def test_stopped_old_service_remains_stopped(self):
        result, state = self.install(active=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(state["persistent"])
        self.assertFalse(state["active"])
        self.assertTrue(state["enabled"])
        self.assertEqual(state["effects"], ["create-account", "reload"])

    def test_persistent_account_reinstall_is_idempotent(self):
        result, state = self.install(dynamic=False, persistent=True, group=True, enabled=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(state["active"])
        self.assertFalse(state["enabled"])
        self.assertEqual(state["effects"], ["reload", "restart"])

    def test_policy_denies_persistent_service_restart(self):
        result, state = self.install(dynamic=False, persistent=True, group=True, deny=["restart"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(state["active"])
        self.assertEqual(state["effects"], ["reload"])
        self.assertEqual(state["policy_checks"], ["restart"])


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--mock":
        sys.exit(run_mock())
    unittest.main(verbosity=2)