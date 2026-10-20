"""Opt-in Windows/OpenSSH-server integration suite. Never edits user profiles.

Uses the embedded agent for every SSH connection. ssh-keygen is used only to
read an existing trusted public host-key record for the transparent fault relay.
"""
import argparse
import asyncio
import json
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import time


class Relay:
    def __init__(self, host, port):
        self.host, self.port = host, port
        self.writers = set()
        self.accepted = 0
        self.blackhole = False

    async def client(self, reader, writer):
        self.accepted += 1
        upstream = None
        try:
            remote, upstream = await asyncio.open_connection(self.host, self.port)
            self.writers.update((writer, upstream))

            async def copy(source, destination):
                while data := await source.read(32768):
                    if not self.blackhole:
                        destination.write(data)
                        await destination.drain()

            tasks = [asyncio.create_task(copy(reader, upstream)),
                     asyncio.create_task(copy(remote, writer))]
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        except (OSError, asyncio.CancelledError):
            pass
        finally:
            for stream in (writer, upstream):
                if stream is not None:
                    self.writers.discard(stream)
                    stream.close()

    def disconnect(self):
        for writer in list(self.writers):
            writer.close()


async def main(args):
    def expected_reset(loop, context):
        error = context.get("exception")
        if isinstance(error, ConnectionResetError) and getattr(error, "winerror", None) == 10054:
            return  # Expected when intentionally breaking the transparent relay.
        loop.default_exception_handler(context)
    asyncio.get_running_loop().set_exception_handler(expected_reset)
    exe = pathlib.Path(args.agent).resolve()
    data = pathlib.Path(args.data).resolve()
    data.mkdir(parents=True, exist_ok=True)
    if (data / "profiles.json").exists():
        raise RuntimeError("Use a fresh test directory; refusing to replace profiles")
    results = []

    def passed(name):
        results.append({"test": name, "result": "passed", "time": time.time()})
        print("PASS", name, flush=True)

    # Re-map an already trusted key to the transparent loopback relay. No TOFU.
    known = pathlib.Path.home() / ".ssh" / "known_hosts"
    keygen = shutil.which("ssh-keygen.exe")
    key_lookup = subprocess.run([keygen, "-F", args.remote_host, "-f", str(known)],
                                capture_output=True, text=True, check=True)
    records = [line.split() for line in key_lookup.stdout.splitlines()
               if line and not line.startswith("#")]
    trusted = next((row[1:3] for row in records if len(row) >= 3 and row[1] == "ssh-ed25519"), None)
    if not trusted:
        raise RuntimeError("An existing trusted Ed25519 server key is required")
    relay = Relay(args.remote_host, args.remote_port)
    server = await asyncio.start_server(relay.client, "127.0.0.1", 0)
    relay_port = server.sockets[0].getsockname()[1]
    trust_file = data / "known_hosts"
    trust_file.write_text(f"[127.0.0.1]:{relay_port} {' '.join(trusted)}\n", encoding="utf-8")
    probe = ("python3 -c \"import socket; s=socket.create_connection(('127.0.0.1',"
             f"{args.reverse_port}),5); b=s.recv(256); s.close(); "
             "assert b.startswith(b'SSH-2.0-OpenSSH_for_Windows'),b\"")
    base = dict(host="integration", hostname="127.0.0.1", user=args.user,
                port=relay_port, identityFile=str(pathlib.Path(args.key).resolve()),
                knownHosts=str(trust_file), target="127.0.0.1", targetPort=22,
                retry=True, autoConnect=True)
    profiles = [dict(base, id="reverse", name="Reverse integration", remotePort=args.reverse_port,
                     direction="reverse", probe=probe),
                dict(base, id="local", name="Local integration", remotePort=args.local_port,
                     direction="local")]
    (data / "profiles.json").write_text(json.dumps(profiles), encoding="utf-8")
    agent = await asyncio.create_subprocess_exec(str(exe), "--data-dir", str(data),
                                                "--agent", "--no-tray", "--test-mode",
                                                creationflags=subprocess.CREATE_NO_WINDOW)

    async def ctl(op, **fields):
        process = await asyncio.create_subprocess_exec(
            str(exe), "--data-dir", str(data), "--ctl", json.dumps(dict(op=op, **fields)),
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW)
        output, error = await asyncio.wait_for(process.communicate(), 25)
        if process.returncode:
            raise RuntimeError(f"IPC {op} failed: {error.decode(errors='replace')} {output.decode(errors='replace')}")
        return json.loads(output)["snapshot"]

    async def wait(predicate, limit=30):
        deadline = time.monotonic() + limit
        last = None
        while time.monotonic() < deadline:
            try:
                last = await ctl("snapshot")
                if predicate(last):
                    return last
            except RuntimeError:
                if agent.returncode is not None:
                    raise
            await asyncio.sleep(.25)  # Test orchestration only; never production polling.
        raise AssertionError(f"State deadline expired: {last}")

    async def healthy(limit=90):
        return await wait(lambda s: s["connections"]["reverse"]["state"] == "verified"
                          and s["connections"]["local"]["state"] == "forwarding", limit)

    async def banner():
        reader, writer = await asyncio.open_connection("127.0.0.1", args.local_port)
        value = await asyncio.wait_for(reader.readline(), 8)
        writer.close()
        await writer.wait_closed()
        assert value.startswith(b"SSH-2.0-"), value

    try:
        await healthy(); await banner(); passed("simultaneous reverse/local forwarding; verified PC return path")
        assert agent.returncode is None
        passed("UI-independent operation without a frontend process")
        duplicate = await asyncio.create_subprocess_exec(str(exe), "--data-dir", str(data),
                                                         "--agent", "--no-tray",
                                                         creationflags=subprocess.CREATE_NO_WINDOW)
        assert await asyncio.wait_for(duplicate.wait(), 8) == 0
        assert (await ctl("snapshot"))["agentPid"] == agent.pid
        passed("duplicate launch attaches safely without replacing the agent")
        # Abort real local-forward clients after reading the remote banner. These
        # must close only their own channels, with no SSH transport reconnection.
        prior = await ctl("snapshot")
        def reset_client():
            with socket.create_connection(("127.0.0.1", args.local_port), timeout=8) as stream:
                assert stream.recv(256).startswith(b"SSH-2.0-")
                stream.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("HH", 1, 0))
        for _ in range(10):
            await asyncio.to_thread(reset_client)
        await wait(lambda s: sum(entry["code"] == "stream_reset"
                                for entry in s["connections"]["local"]["logs"]) >= 10)
        await healthy(); await banner()
        after = await ctl("snapshot")
        assert after["agentPid"] == prior["agentPid"]
        assert all(after["connections"][key]["retries"] == prior["connections"][key]["retries"]
                   for key in ("reverse", "local"))
        passed("repeated TCP resets are isolated; same agent and SSH sessions remain healthy")
        before = relay.accepted
        relay.disconnect()
        await wait(lambda s: any(c["retries"] for c in s["connections"].values()))
        await healthy(); await banner()
        assert relay.accepted >= before + 2
        passed("forced TCP loss; both independent tunnels reconnect")
        await ctl("test_recovery", enabled=False)
        await wait(lambda s: all(c["state"] == "suspended" for c in s["connections"].values()))
        await ctl("test_recovery", enabled=True)
        await healthy(); await banner(); passed("suspend/resume event path; sockets released and restored")

        unknown = dict(base, id="unknown", name="Unknown key", remotePort=args.reverse_port+1,
                       autoConnect=False, knownHosts=str(data / "empty_known_hosts"))
        await ctl("configure", profiles=profiles+[unknown]); await ctl("connect", id="unknown")
        await wait(lambda s: s["connections"]["unknown"]["error"] is not None and
                   s["connections"]["unknown"]["error"]["code"] == "host_key_unknown")
        passed("unknown host key rejected without retry or acceptance")
        changed_key = next((row[1:3] for line in known.read_text().splitlines()
                            if (row := line.split()) and len(row) >= 3 and row[1] == "ssh-ed25519"
                            and row[2] != trusted[1]), None)
        if not changed_key:
            raise RuntimeError("A second existing public key is needed for the changed-key test")
        bad = data / "changed_known_hosts"
        bad.write_text(f"[127.0.0.1]:{relay_port} {' '.join(changed_key)}\n")
        changed = dict(unknown, id="changed", name="Changed key", knownHosts=str(bad))
        await ctl("configure", profiles=profiles+[changed]); await ctl("connect", id="changed")
        await wait(lambda s: s["connections"]["changed"]["error"] is not None and
                   s["connections"]["changed"]["error"]["code"] == "host_key_changed")
        passed("changed host key rejected; healthy connections preserved")
        await ctl("configure", profiles=profiles)
        if args.blackhole:
            relay.blackhole = True
            await wait(lambda s: all(c["state"] == "retrying" for c in s["connections"].values()), 110)
            relay.blackhole = False; relay.disconnect()
            await healthy(); await banner(); passed("silent network blackhole detected by SSH keepalives")
        if args.physical_sleep:
            previous = (await healthy())["connections"]["reverse"]["verifiedAt"]
            sleeper = await asyncio.create_subprocess_exec(
                "pwsh.exe", "-NoProfile", "-File", str(pathlib.Path(__file__).with_name("test-sleep.ps1")),
                "-ResultPath", str(data / "sleep-result.json"))
            assert await asyncio.wait_for(sleeper.wait(), 100) == 0, "Physical sleep test failed; inspect power evidence"
            await healthy(); await banner()
            await wait(lambda s: s["connections"]["reverse"]["verifiedAt"] is not None and
                       s["connections"]["reverse"]["verifiedAt"] > previous, 90)
            passed("physical Windows sleep/resume; agent and both routes recovered")
        await ctl("disconnect_all")
        await wait(lambda s: all(c["state"] == "stopped" for c in s["connections"].values()))
        try:
            reader, writer = await asyncio.open_connection("127.0.0.1", args.local_port)
        except OSError:
            pass
        else:
            writer.close(); raise AssertionError("Local listener survived disconnect")
        passed("graceful disconnect releases local listener and forwarded streams")
        await ctl("shutdown")
        await asyncio.wait_for(agent.wait(), 10)
        assert agent.returncode == 0
        passed("acknowledged graceful agent shutdown")
    finally:
        if agent.returncode is None:
            try: await ctl("shutdown")
            except Exception: agent.terminate()
            try: await asyncio.wait_for(agent.wait(), 10)
            except asyncio.TimeoutError: agent.kill()
        relay.disconnect(); server.close()
        (data / "results.json").write_text(json.dumps(results, indent=2))
        # Python 3.13's Windows proactor can fail to detach a reset TCP transport
        # from Server during forced-loss tests. Never let test cleanup hang.
        try:
            await asyncio.wait_for(server.wait_closed(), 3)
        except asyncio.TimeoutError:
            print("Relay cleanup deadline reached after forced TCP resets", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", default="build/native/Connecty.Agent.exe")
    parser.add_argument("--data", required=True)
    parser.add_argument("--remote-host", required=True)
    parser.add_argument("--remote-port", type=int, default=22)
    parser.add_argument("--user", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--reverse-port", type=int, default=22031)
    parser.add_argument("--local-port", type=int, default=22631)
    parser.add_argument("--blackhole", action="store_true")
    parser.add_argument("--physical-sleep", action="store_true", help="Opt-in real sleep with a 30-second wake timer")
    asyncio.run(main(parser.parse_args()))
