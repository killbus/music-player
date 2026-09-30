#!/usr/bin/env python3
"""Install the real DEB on an ephemeral systemd CI runner and test lifecycle."""
import array
import json
import math
import os
from pathlib import Path
import subprocess
import threading
import time
import urllib.error
import urllib.request
import wave

UNIT = 'music-player.service'
STATE = Path('/var/lib/music-player')
DEFAULTS = Path('/etc/default/music-player')
DROPIN = Path('/etc/systemd/system/music-player.service.d/ci.conf')
HTTP_PORT = 5053


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT, timeout=180).strip()


def active():
    return subprocess.run(['systemctl', 'is-active', '--quiet', UNIT]).returncode == 0


def graphql(query, variables=None):
    request = urllib.request.Request(f'http://127.0.0.1:{HTTP_PORT}/graphql',
        data=json.dumps({'query': query, 'variables': variables or {}}).encode(),
        headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=3) as response:
        result = json.load(response)
    assert not result.get('errors'), result
    return result['data']


def ready():
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        try:
            tracks = graphql('{ tracks(limit:10) { id title uri discNumber } }')['tracks']
            if tracks:
                return tracks
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.5)
    raise AssertionError('system service/library did not become ready')


def main():
    global HTTP_PORT
    # These fixed system paths are intentional only on disposable GitHub runners.
    assert os.geteuid() == 0 and Path('/run/systemd/system').is_dir()
    assert not STATE.exists() and not DEFAULTS.exists() and not DROPIN.exists(), 'runner is not clean'
    package, = Path('dist').glob('music-player-cli_*.deb')
    evidence = Path('dist/build/evidence')
    evidence.mkdir(parents=True, exist_ok=True)
    report = {}
    pipes = Path('/run/music-player-ci')
    media = Path('/srv/music-player-ci')
    fd = None
    reader_done = threading.Event()
    reader = None
    received = [0]
    original_unprivileged_port = run('sysctl', '-n', 'net.ipv4.ip_unprivileged_port_start')
    try:
        # Ensure port 80 really requires a capability on this runner.
        run('sysctl', '-w', 'net.ipv4.ip_unprivileged_port_start=1024')
        run('apt-get', 'install', '-y', '--no-install-recommends', str(package.resolve()))
        run('systemd-analyze', 'verify', '/usr/lib/systemd/system/' + UNIT)
        assert not active(), 'first install started daemon'
        enabled = subprocess.run(['systemctl', 'is-enabled', UNIT], text=True, capture_output=True)
        assert enabled.stdout.strip() == 'disabled', enabled
        report['first_install_disabled'] = True
        pipes.mkdir(mode=0o755)
        media.mkdir(mode=0o755)
        fifo = pipes / 'probe.fifo'
        os.mkfifo(fifo, 0o666)
        fifo.chmod(0o666)
        fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

        def drain():
            while not reader_done.is_set():
                try:
                    chunk = os.read(fd, 65536)
                    if any(chunk):
                        received[0] += len(chunk)
                except BlockingIOError:
                    reader_done.wait(0.01)

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        samples = array.array('h', (int(9000 * math.sin(2 * math.pi * 440 * i / 44100))
                                   for i in range(44100 * 8) for _ in range(2)))
        with wave.open(str(media / 'probe.wav'), 'wb') as output:
            output.setparams((2, 2, 44100, 0, 'NONE', 'not compressed'))
            output.writeframes(samples.tobytes())
        DEFAULTS.write_text(f'MUSIC_PLAYER_AUDIO_OUTPUT=fifo:{fifo}\n'
                            f'MUSIC_PLAYER_MUSIC_DIRECTORY={media}\n'
                            'MUSIC_PLAYER_ATPROTO=false\nMUSIC_PLAYER_SCROBBLE=false\n'
                            'MUSIC_PLAYER_REMOTE_PLAYER=false\n')
        DROPIN.parent.mkdir(parents=True)
        DROPIN.write_text(f'[Service]\nReadWritePaths={fifo}\n')
        run('systemctl', 'daemon-reload')
        run('systemctl', 'enable', '--now', UNIT)
        tracks = ready()
        pid = run('systemctl', 'show', '-p', 'MainPID', '--value', UNIT)
        assert int(run('ps', '-o', 'uid=', '-p', pid)) != 0, 'service runs as root'
        graphql('mutation($track:TrackInput!) { addTrack(track:$track) { id } }', {'track': tracks[0]})
        graphql('mutation { play }')
        deadline = time.monotonic() + 20
        while received[0] < 44100 and time.monotonic() < deadline:
            time.sleep(0.05)
        assert received[0] >= 44100, 'service could not write decoded audio to external FIFO'
        graphql('mutation { stop }')
        report.update(nonroot=True, fifo_pcm_bytes=received[0])
        with DEFAULTS.open('a') as output:
            output.write('MUSIC_PLAYER_HTTP_PORT=80\n')
        HTTP_PORT = 80
        run('systemctl', 'restart', UNIT)
        ready()
        pid = run('systemctl', 'show', '-p', 'MainPID', '--value', UNIT)
        assert int(run('ps', '-o', 'uid=', '-p', pid)) != 0, 'port 80 service runs as root'
        status = Path(f'/proc/{pid}/status').read_text()
        caps = next(line.split()[1] for line in status.splitlines() if line.startswith('CapEff:'))
        assert int(caps, 16) & (1 << 10), 'missing CAP_NET_BIND_SERVICE'
        report['nonroot_http_port_80'] = True
        settings = STATE / 'config/music-player/settings.toml'
        with settings.open('a') as output:
            output.write('\n# service persistence probe\n')
        defaults_before = DEFAULTS.read_bytes()
        # Reinstall takes the same dpkg upgrade maintscript path as a new version.
        run('dpkg', '-i', str(package))
        ready()
        assert pid != run('systemctl', 'show', '-p', 'MainPID', '--value', UNIT)
        assert run('systemctl', 'is-enabled', UNIT) == 'enabled'
        assert '# service persistence probe' in settings.read_text()
        assert DEFAULTS.read_bytes() == defaults_before
        report['active_upgrade_restarts_and_preserves_state'] = True
        # Enabled but deliberately stopped must stay stopped across upgrade.
        run('systemctl', 'stop', UNIT)
        run('dpkg', '-i', str(package))
        assert not active() and run('systemctl', 'is-enabled', UNIT) == 'enabled'
        report['stopped_upgrade_stays_stopped'] = True
        run('systemctl', 'disable', UNIT)
        run('systemctl', 'start', UNIT)
        ready()
        pid = run('systemctl', 'show', '-p', 'MainPID', '--value', UNIT)
        run('dpkg', '-i', str(package))
        ready()
        assert pid != run('systemctl', 'show', '-p', 'MainPID', '--value', UNIT)
        assert subprocess.run(['systemctl', 'is-enabled', '--quiet', UNIT]).returncode != 0
        report['disabled_running_upgrade_restarts_without_enabling'] = True
        run('systemctl', 'enable', UNIT)
        run('dpkg', '--remove', 'music-player-cli')
        assert not active() and settings.exists()
        assert DEFAULTS.read_bytes() == defaults_before
        run('dpkg', '--purge', 'music-player-cli')
        assert settings.exists() and not DEFAULTS.exists()
        assert not os.path.lexists('/etc/systemd/system/multi-user.target.wants/' + UNIT)
        report['remove_stops_purge_retains_data'] = True
        print(json.dumps(report))
    finally:
        run('sysctl', '-w', f'net.ipv4.ip_unprivileged_port_start={original_unprivileged_port}')
        subprocess.run(['systemctl', 'stop', UNIT], check=False)
        reader_done.set()
        if reader:
            reader.join(timeout=2)
        if fd is not None:
            os.close(fd)
        log = subprocess.run(['journalctl', '-u', UNIT, '--no-pager'], text=True, capture_output=True)
        arch = os.environ['ARCH']
        (evidence / f'systemd-{arch}.log').write_text(log.stdout + log.stderr)
        (evidence / f'systemd-{arch}.json').write_text(json.dumps(report, indent=2) + '\n')
        # Keep failures' state for runner diagnostics; GitHub disposes of the VM.


if __name__ == '__main__':
    main()
