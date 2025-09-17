iperf_ns.sh
============

Interactive helper to measure throughput between two network namespaces using iperf3.

Usage
-----

- Ensure you have root privileges and iperf3 installed.
- Make the script executable: `chmod +x scripts/iperf_ns.sh`
- Run it with sudo: `sudo scripts/iperf_ns.sh`

What it does
------------

1. Prompts for a source namespace name and a destination namespace name.
2. Verifies both namespaces exist (via `ip netns list`).
3. Starts `iperf3` in server mode inside the destination namespace.
4. Runs `iperf3` client from the source namespace against the server for 10 seconds.
5. Cleans up the server process and leaves a server log in `/tmp/`.

Notes
-----
- The script connects to 127.0.0.1 inside the namespaces; ensure the namespaces' loopback interfaces are up and routes are configured if necessary.
- If your topology uses different IPs between namespaces, edit `iperf_ns.sh` to use the proper destination IP.

Install helper
--------------

Make all scripts executable and symlink them into `~/.local/bin` (recommended):

	chmod +x scripts/install-scripts.sh
	./scripts/install-scripts.sh

This will create simple command names (for example `iperf_ns.sh` becomes `iperf_ns`) in `~/.local/bin`. Add that directory to your PATH if needed.
