#!/usr/bin/env python3
"""Test an actual packaged daemon, Web UI, persistence and FIFO PCM on Linux."""
import array
import json
import math
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
import wave


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT, timeout=60).strip()


def main():
    image, arch = sys.argv[1:]
    name = 'music-player-test-' + uuid.uuid4().hex[:10]
    volume = name + '-data'
    evidence = Path('dist/build/evidence')
    evidence.mkdir(parents=True, exist_ok=True)
    report = {'image': image, 'architecture': arch}
    reader_done = threading.Event()
    received = {'bytes': 0, 'nonzero': 0}
    fd = None
    reader = None
    try:
        with tempfile.TemporaryDirectory(prefix='music-player-smoke-') as tmp:
            root = Path(tmp)
            root.chmod(0o755)
            music = root / 'music'
            pipes = root / 'pipes'
            music.mkdir()
            pipes.mkdir()
            fifo = pipes / 'test.fifo'
            os.mkfifo(fifo, 0o666)
            fifo.chmod(0o666)
            # A deterministic audible tone; no codecs or external stream needed.
            tone = array.array('h', (int(9000 * math.sin(2 * math.pi * 440 * i / 44100))
                                    for i in range(44100 * 8) for _ in range(2)))
            if sys.byteorder != 'little':
                tone.byteswap()
            with wave.open(str(music / 'probe.wav'), 'wb') as output:
                output.setparams((2, 2, 44100, 0, 'NONE', 'not compressed'))
                output.writeframes(tone.tobytes())
            fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

            def drain():
                with selectors.DefaultSelector() as selector:
                    selector.register(fd, selectors.EVENT_READ)
                    while not reader_done.is_set():
                        for _key, _events in selector.select(timeout=0.1):
                            try:
                                data = os.read(fd, 65536)
                            except BlockingIOError:
                                continue
                            received['bytes'] += len(data)
                            if any(data):
                                received['nonzero'] += len(data)

            reader = threading.Thread(target=drain, daemon=True)
            reader.start()
            assert run('docker', 'image', 'inspect', image, '--format', '{{.Architecture}}') == arch
            assert run('docker', 'image', 'inspect', image, '--format', '{{.Config.User}}') == '10001:10001'
            run('docker', 'run', '-d', '--name', name,
                '--mount', f'type=volume,source={volume},target=/data',
                '--mount', f'type=bind,source={music},target=/music,readonly',
                '--mount', f'type=bind,source={pipes},target=/run/snapcast',
                '-p', '127.0.0.1::5053',
                '-e', 'MUSIC_PLAYER_AUDIO_OUTPUT=fifo:/run/snapcast/test.fifo',
                '-e', 'MUSIC_PLAYER_ATPROTO=false', '-e', 'MUSIC_PLAYER_SCROBBLE=false',
                '-e', 'MUSIC_PLAYER_REMOTE_PLAYER=false', image)
            host = run('docker', 'port', name, '5053/tcp')
            url = 'http://' + host

            def graphql(query, variables=None):
                request = urllib.request.Request(url + '/graphql',
                    data=json.dumps({'query': query, 'variables': variables or {}}).encode(),
                    headers={'Content-Type': 'application/json'})
                with urllib.request.urlopen(request, timeout=3) as response:
                    result = json.load(response)
                if result.get('errors'):
                    raise RuntimeError(result['errors'])
                return result['data']

            def wait_for_library():
                deadline = time.monotonic() + 60
                last_error = None
                while time.monotonic() < deadline:
                    try:
                        result = graphql('{ tracks(limit:10) { id title uri discNumber } }')['tracks']
                        if result:
                            return result
                    except (OSError, urllib.error.URLError) as error:
                        last_error = error
                    time.sleep(0.5)
                raise AssertionError(f'daemon/library readiness timed out: {last_error}')

            tracks = wait_for_library()
            with urllib.request.urlopen(url, timeout=5) as response:
                html = response.read().lower()
                assert response.status == 200 and b'<html' in html, 'embedded Web UI missing'
            canonical = json.loads(Path(f'dist/build/release/build-{arch}.json').read_text())
            packaged_hash = run('docker', 'exec', name, 'sha256sum', '/usr/bin/music-player').split()[0]
            assert packaged_hash == canonical['binary_sha256'], 'container binary differs'
            run('docker', 'exec', name, 'sh', '-ec',
                'test ! -e /usr/bin/music-player-desktop; test -s /data/config/music-player/music-player.sqlite3')
            graphql('mutation($track:TrackInput!) { addTrack(track:$track) { id } }', {'track': tracks[0]})
            graphql('mutation { play }')
            deadline = time.monotonic() + 20
            while received['nonzero'] < 44100 and time.monotonic() < deadline:
                time.sleep(0.1)
            assert received['nonzero'] >= 44100, f'no decoded tone reached FIFO: {received}'
            report['pcm'] = received.copy()
            graphql('mutation { stop }')
            # Exercise the volume with a real settings file, not just a marker elsewhere.
            settings = '/data/config/music-player/settings.toml'
            before = run('docker', 'exec', name, 'cat', settings)
            run('docker', 'exec', name, 'sh', '-ec',
                'printf "\\n# distribution persistence probe\\n" >> /data/config/music-player/settings.toml')
            run('docker', 'restart', name)
            wait_for_library()
            after = run('docker', 'exec', name, 'cat', settings)
            assert before in after and '# distribution persistence probe' in after
            logs = run('docker', 'logs', name)
            assert 'Failed to open audio engine' not in logs and 'Invalid audio_output' not in logs
            report.update(nonroot=True, webui=True, binary_identity=True, persisted=True)
            print(json.dumps(report))
    finally:
        reader_done.set()
        if reader:
            reader.join(timeout=2)
        if fd is not None:
            os.close(fd)
        log = subprocess.run(['docker', 'logs', name], capture_output=True, text=True)
        suffix = image.replace(':', '-').replace('/', '-')
        (evidence / f'{suffix}.log').write_text(log.stdout + log.stderr)
        (evidence / f'{suffix}.json').write_text(json.dumps(report, indent=2) + '\n')
        subprocess.run(['docker', 'rm', '-f', name], stdout=subprocess.DEVNULL, check=False)
        subprocess.run(['docker', 'volume', 'rm', volume], stdout=subprocess.DEVNULL, check=False)


if __name__ == '__main__':
    main()
