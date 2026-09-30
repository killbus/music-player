#!/usr/bin/env python3
"""Linux-only FIFO output backpressure harness (CI-only)."""
import hashlib
import http.server
import json
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
from run import mp3  # noqa: E402

BLOCK_TIMEOUT = 10.0
WATCHDOG_TIMEOUT = 5.0
JOIN_TIMEOUT = 2000.0
RELEASE_KEYS = ("cancel_requested", "reader_released",
                "decoder_joined", "worker_exited")


def run_harness(exe_path):
    workdir = tempfile.TemporaryDirectory(prefix="emby-fifo-probe-")
    base = Path(workdir.name)
    fixture_path = base / "tone-30s.mp3"
    media = mp3(fixture_path, 30)
    fifo_path = base / "pcm.fifo"
    os.mkfifo(fifo_path)
    reader_fd = None
    server = None
    server_thread = None
    proc = None
    reader_threads = []
    start = time.monotonic()
    engine_events = []
    stderr_lines = []
    parse_errors = []
    server_errors = []
    observer_events = []
    http_requests = []
    watchdog_killed = False

    def stamp(event, **data):
        entry = {"ms": round((time.monotonic() - start) * 1000, 2),
                 "event": event, **data}
        observer_events.append(entry)
        return entry

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def do_GET(self):
            http_requests.append({"path": self.path})
            try:
                self.send_response(200)
                self.send_header("Content-Type", "audio/mpeg")
                self.send_header("Content-Length", str(len(media)))
                self.end_headers()
                self.wfile.write(media)
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError,
                    OSError) as error:
                server_errors.append(type(error).__name__)

    try:
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.daemon_threads = True
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()
        # Open the read end non-blocking first and keep this descriptor open
        # until after the player process exits: the player therefore blocks in
        # its writer while a real FIFO reader exists but never consumes.
        reader_fd = os.open(fifo_path, os.O_RDONLY | os.O_NONBLOCK)
        url = f"http://127.0.0.1:{server.server_port}/finite_no_range.mp3"
        proc = subprocess.Popen([str(exe_path), url, "fifo:" + str(fifo_path)],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True)

        def collect(source, sink):
            for line in source:
                sink.append(line.rstrip("\n"))

        threads = [threading.Thread(target=collect, args=(proc.stdout, engine_events)),
                   threading.Thread(target=collect, args=(proc.stderr, stderr_lines))]
        reader_threads = threads
        for thread in threads:
            thread.start()

        def parsed():
            values = []
            for line in engine_events:
                try:
                    values.append(json.loads(line))
                except json.JSONDecodeError:
                    if line not in parse_errors:
                        parse_errors.append(line)
            return values

        def is_blocked(event):
            detail = event.get("detail", {})
            return (event.get("event") == "output"
                    and detail.get("nonblocking") is True
                    and detail.get("backpressure_events", 0) > 0
                    and detail.get("bytes_written", 0) > 0
                    and detail.get("nonzero_bytes_written", 0) > 0)

        blocked_event = None
        deadline = time.monotonic() + BLOCK_TIMEOUT
        while time.monotonic() < deadline:
            for event in parsed():
                if is_blocked(event):
                    blocked_event = event
                    break
            if blocked_event is not None:
                break
            time.sleep(0.05)
        drop_sent = None
        if blocked_event is not None:
            # The receiver still never reads. Observe the media clock once
            # actual kernel backpressure has persisted beyond one output chunk.
            time.sleep(0.3)
            drop_sent = round((time.monotonic() - start) * 1000, 2)
            proc.stdin.write("drop\n")
            proc.stdin.flush()
        # Five seconds is cleanup only; the two-second gate is validated from
        # child event timestamps after the collector threads are joined.
        try:
            proc.wait(timeout=WATCHDOG_TIMEOUT)
        except subprocess.TimeoutExpired:
            watchdog_killed = True
            stamp("watchdog_kill")
            proc.kill()
            proc.wait(timeout=WATCHDOG_TIMEOUT)
        for thread in reader_threads:
            thread.join(timeout=2.0)

        drop_joined_at = next((e for e in parsed()
                               if e.get("event") == "drop_joined"), None)
        drop_called_at = next((e for e in parsed()
                               if e.get("event") == "stop_called"
                               and e.get("detail", {}).get("command") == "drop"), None)
        drop_latency = (drop_joined_at["ms"] - drop_called_at["ms"]
                        if drop_joined_at and drop_called_at else None)
        sessions = [e for e in parsed() if e.get("event") == "session"]
        final_session = sessions[-1] if sessions else None
        final_detail = final_session.get("detail", {}) if final_session else {}
        final_session_released = (final_session is not None
                                  and all(final_detail.get(key) is True
                                          for key in RELEASE_KEYS))
        output_events = [e for e in parsed() if e.get("event") == "output"]
        last_output = output_events[-1:]
        last_detail = last_output[0]["detail"] if last_output else {}
        bytes_written = last_detail.get("bytes_written", 0)
        nonzero_bytes_written = last_detail.get("nonzero_bytes_written", 0)
        blocked_count = (blocked_event["detail"]["backpressure_events"]
                         if blocked_event else 0)
        clocks = [e["detail"] for e in sessions]
        blocked_clocks = [e["detail"] for e in sessions
                          if blocked_event is not None and drop_called_at is not None
                          and blocked_event["ms"] + 50 <= e["ms"] < drop_called_at["ms"]]
        frames = [c.get("output_frames", -1) for c in clocks]
        clock_schema = bool(clocks) and all(
            c.get("output_boundary") == "ByteStream"
            and c.get("output_generation") == c.get("generation")
            and c.get("output_sample_rate") == 44100
            and isinstance(c.get("output_frames"), int)
            and c["output_frames"] >= 0
            and c.get("output_duration_ns") == c["output_frames"] * 1_000_000_000 // 44100
            for c in clocks)
        fixture_sha256 = hashlib.sha256(fixture_path.read_bytes()).hexdigest()
        exe_sha256 = hashlib.sha256(exe_path.read_bytes()).hexdigest()
        checks = {
            "backpressure_blocked": blocked_event is not None and blocked_count > 0,
            "blocked_nonblocking": blocked_event is not None
                                   and blocked_event.get("detail", {}).get("nonblocking") is True,
            "nonzero_bytes_written": bytes_written > 0 and nonzero_bytes_written > 0,
            "session_present": len(sessions) >= 1,
            "clock_schema": clock_schema,
            "clock_monotonic": bool(frames) and frames == sorted(frames),
            "clock_excludes_silence": bool(frames) and 0 < frames[-1] * 4 <= bytes_written,
            "clock_frozen_while_blocked_and_after_drop": len(blocked_clocks) >= 3
                and all(c.get("output_frames") == frames[-1] for c in blocked_clocks),
            "final_session_released": final_session_released,
            "exit_zero": proc.returncode == 0,
            "no_watchdog": not watchdog_killed,
            "no_stderr": not stderr_lines,
            "no_server_errors": not server_errors,
            "no_parse_errors": not parse_errors,
            "drop_joined": drop_joined_at is not None,
            "drop_latency": drop_latency is not None and 0 <= drop_latency <= JOIN_TIMEOUT,
            "one_http_request": len(http_requests) == 1,
            "collectors_joined": all(not thread.is_alive() for thread in reader_threads),
            "no_output_failure": bool(output_events) and all(
                e["detail"].get("output_failed") is False
                and e["detail"].get("writer_exited") is False for e in output_events),
        }
        evidence = {
            "summary": {"result": "Pass" if all(checks.values()) else "Fail",
                        "checks": checks,
                        "blocked_event": blocked_event,
                        "last_output_event": last_output[0] if last_output else None,
                        "drop_called": drop_called_at,
                        "drop_joined": drop_joined_at,
                        "drop_join_latency_ms": drop_latency,
                        "drop_sent_observer_ms": drop_sent,
                        "session_count": len(sessions),
                        "output_frames": frames[-1] if frames else None,
                        "blocked_clock_samples": len(blocked_clocks),
                        "final_session": final_session,
                        "final_session_released": final_session_released,
                        "watchdog_killed": watchdog_killed,
                        "stderr": stderr_lines,
                        "server_errors": server_errors,
                        "parse_errors": parse_errors,
                        "exit_code": proc.returncode,
                        "http_request_count": len(http_requests),
                        "http_requests": http_requests,
                        "fixture_sha256": fixture_sha256,
                        "executable_sha256": exe_sha256,
                        "platform": platform.platform()},
            "engine_events": parsed(),
            "raw_stdout": engine_events,
            "observer_events": observer_events,
            "stderr": stderr_lines,
            "http_requests": http_requests,
            "fifo_reader_fd": {"mode": "O_RDONLY|O_NONBLOCK",
                               "kept_open_until_process_exit": True},
            "note": ("FIFO read fd was opened non-blocking and deliberately never "
                     "read; it stays open across the whole child lifetime so "
                     "backpressure removal cannot fake a pass. output counters are "
                     "byte-stream delivery observations, not consumed "
                     "time. the two-second exit gate is validated from child event "
                     "timestamps; the five-second watchdog is cleanup only.")}
        results_dir = ROOT / "results"
        results_dir.mkdir(parents=True, exist_ok=True)
        (results_dir / "output-fifo.json").write_text(
            json.dumps(evidence, indent=2), encoding="utf-8")
        print(json.dumps(evidence["summary"]))
        return 0 if all(checks.values()) else 1
    finally:
        if proc is not None and proc.poll() is None:
            proc.kill()
            try:
                proc.wait(timeout=WATCHDOG_TIMEOUT)
            except subprocess.TimeoutExpired:
                pass
        for thread in reader_threads:
            thread.join(timeout=2.0)
        if proc is not None:
            for stream in (proc.stdin, proc.stdout, proc.stderr):
                try:
                    stream.close()
                except OSError:
                    pass
        if server is not None:
            server.shutdown()
            server.server_close()
        if server_thread is not None:
            server_thread.join(timeout=2.0)
        if reader_fd is not None:
            os.close(reader_fd)
        workdir.cleanup()


def main():
    if platform.system() != "Linux":
        raise RuntimeError("Linux-only FIFO harness")
    suffix = ".exe" if platform.system() == "Windows" else ""
    exe_path = ROOT / ("target/debug/emby-runtime-probe" + suffix)
    raise SystemExit(run_harness(exe_path))

if __name__ == "__main__":
    main()
