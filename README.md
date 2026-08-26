# ostrail

[![Crates.io](https://img.shields.io/crates/v/ostrail.svg)](https://crates.io/crates/ostrail)
[![CI](https://github.com/wabuntu/ostrail/actions/workflows/rust.yml/badge.svg)](https://github.com/wabuntu/ostrail/actions/workflows/rust.yml)

Give `ostrail` a request ID (`req-...`) or a resource UUID (a server,
volume, network, ...), and it SSHes into the hosts running your
OpenStack services, pulls their journals for that time window, and
prints every matching log line from every service — Nova, Neutron,
Cinder, Keystone, whatever's out there — merged into one chronological,
color-coded timeline. No more guessing which of a dozen services to
check first, or which host it happened on.

```
$ ostrail 7ab93819-ede2-4c95-b5c0-ff524d0c3081 --since "15 minutes ago"
```

```
Searching 1 host(s) for '7ab93819-ede2-4c95-b5c0-ff524d0c3081' (since 15 minutes ago, until now) ...
3 matching line(s):

08-26 14:23:39  devstack     nova-conductor            WARN   WARNING nova.scheduler.utils [None req-3ccf19e4-... admin admin] [instance: 7ab93819-...] Setting instance to ERROR state.: nova.exception_Remote.NoValidHost_Remote: No valid host was found.
08-26 14:24:58  devstack     devstack@neutron-api.se…  ?      [pid: 380278|app: 0|req: 26/51] 192.168.122.38 () ... GET /networking/v2.0/ports?fields=security_groups&fields=device_id&device_id=7ab93819-... => generated 12 bytes in 57 msecs (HTTP/1.1 200)
08-26 14:24:59  devstack     devstack@n-api.service    ?      [pid: 379649|app: 0|req: 9/17] 192.168.122.1 () ... GET /compute/v2.1/servers/7ab93819-... => generated 3617 bytes in 165 msecs (HTTP/1.1 200)
```

(Real output, lightly trimmed for width, from an actual `NoValidHost`
scheduling failure on a live DevStack cloud — the WARNING line is
colored yellow, ERROR lines red, in a real terminal.)

## How it works

OpenStack already threads a request ID through every service a call
touches — that's not something ostrail adds, it's a real, existing
`global_request_id` mechanism in `oslo.log`. A request into Nova that
in turn calls Neutron shows up in *both* services' logs tagged with the
same ID (Neutron additionally logs its own local request ID alongside
it). Resource UUIDs work too, just as plain substrings in the log
message rather than a structured tag — Nova tags lines `[instance:
<uuid>]`, Neutron logs a port's `device_id` directly in its request
body dump, and so on.

The hard part was never "does the ID exist in the logs" — it's that
there's no API to query them. So ostrail:

1. Logs in to Keystone and asks Nova (`/os-services`) and Neutron
   (`/v2.0/agents`) which hosts are running something, and
2. SSHes to each one and pulls `journalctl -o json` for the given time
   window, then
3. Decodes and filters every line for your ID *after* fetching it —
   not with a remote `grep` on the raw JSON, because journald encodes
   `MESSAGE` as an array of byte values instead of a string whenever a
   line contains raw control bytes, which happens routinely because
   some OpenStack services colorize their output with ANSI codes even
   when journald (not a real terminal) is capturing them. A substring
   that's plainly visible in the decoded text never appears as that
   substring in the *raw* JSON for such a line.
4. Merges every host's results and sorts by timestamp.

No assumption is made about which systemd unit any service runs
under — DevStack, RDO, and Ubuntu-packaged deployments are all known to
name their units differently, so ostrail just reads a host's entire
journal rather than guessing a unit name. That said, **testing so far
has only been against a real DevStack deployment** (which uses its own
generic `devstack@<code>` naming regardless of the underlying distro,
not RHEL's or Ubuntu's own package-native unit names) — the
unit-name-agnostic design is meant to hold up against RDO/RHEL and
Ubuntu-packaged clouds too, but that's an untested expectation, not a
verified one yet.

## Requirements

- A systemd-journald-based deployment (DevStack, RDO/Packstack,
  Ubuntu-packaged bare metal, OpenStack-Ansible on the host layer, ...).
  **Containerized deployments (Kolla-Ansible, etc.) aren't supported
  yet** — a container's logs usually don't reach the host's own
  journal, so there's nothing for `journalctl` to find there.
- SSH access to the hosts (key-based, already working - ostrail
  doesn't manage credentials for this part), and `sudo journalctl`
  access on each one. Production clouds routinely restrict this more
  tightly than a DevStack box does; if a host isn't reachable this
  way, ostrail reports it and moves on rather than pretending nothing
  happened there.

## Usage

```
$ ostrail <ID>                                    # auto-discover hosts via Nova/Neutron
$ ostrail <ID> --hosts controller,compute-1        # search specific hosts instead
$ ostrail <ID> --since "1 hour ago" --until "30 minutes ago"
$ ostrail <ID> --min-level debug                   # include everything, not just warning+
```

Auto-discovery needs OpenStack credentials the same way the
`openstack` CLI does — `OS_*` environment variables (source your
`openrc.sh`) or `clouds.yaml`. There's no setup wizard here; if neither
is found, pass `--hosts` directly and skip auth entirely.

Flags:

- `--since <spec>` / `--until <spec>`: the search window, in
  `journalctl`'s own time syntax (default: the last 10 minutes)
- `--hosts <a,b,c>`: search these hosts instead of discovering them
- `--min-level <debug|info|warning|error>`: hide log lines below this
  level (default: `warning`). Lines ostrail couldn't classify (not in
  `oslo.log`'s usual `LEVEL logger [...] message` shape - access logs,
  third-party libraries, ...) are always shown regardless, since they
  still matched the search and there's no safe way to guess their
  severity.

## Install

- Cargo: `cargo install ostrail`
- Debian package: https://github.com/wabuntu/ostrail/tree/main/target/debian
- RPM package: https://github.com/wabuntu/ostrail/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/ostrail/tree/main/binaries
