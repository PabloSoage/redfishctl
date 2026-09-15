# redfishctl

Control and monitor a server's BMC from the terminal, over Redfish.

Power the machine on and off, watch every sensor it exposes, chart consumption over time, and
read the event log — without opening the BMC's web interface and without `ipmitool` installed.

Run it with no arguments and you get an interactive interface. Nothing to memorise: the tabs are
one keystroke apart and the keys for the current tab are always on screen.

```
 1 Overview  2 Sensors  3 Charts  4 Power  5 Logs

 Machine                             Attention (1)
   power        On                     FAULT  GPU_FAN01   0 RPM
   machine      GIGABYTE T181-G20      warn   GPU_RFAN05  1050 RPM
   cpus         2  Xeon Platinum 8171M
   memory       96 GiB
   consumption  904 W

 a all  t temps  f fans  v volts  p problems  enter chart  e edit threshold
```

The subcommands below still exist, because scripts need them.

```
$ redfishctl status

  power        On
  health       OK
  machine      GIGABYTE T181-G20
  bios         R25
  cpus         2  Intel(R) Xeon(R) Platinum 8171M CPU @ 2.60GHz
  memory       96 GiB
  consumption  904 W   (avg 913, 273-1330 over 126 min)
  hottest      PCH_TEMP 44 C
  fans         42 present, all OK
```

## Why Redfish and not IPMI

Redfish is plain HTTPS and JSON, so this is a single self-contained binary: no `ipmitool`, no
RMCP+ session, no platform-specific packaging. It works the same on Windows, Linux and macOS.

It also does **not** scrape the BMC's web interface. That interface is itself a Redfish client,
and scraping it breaks on every firmware update.

The trade-off is that a few things still live only in IPMI — the system event log on many
boards, and Serial-over-LAN. For those, `ipmitool` remains the tool.

## Install

```sh
sh scripts/install.sh          # Linux, macOS
```

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1    # Windows
```

Both build and put `redfishctl` in `~/.cargo/bin`, which rustup already has on
your PATH. Or grab a binary from the releases page.

## Setup

```sh
redfishctl config set --host 192.168.1.135 --user admin
```

It prompts for the password without echoing it, and writes everything to a config file
(`redfishctl config path` tells you where) with owner-only permissions on Unix.

Then check it:

```sh
redfishctl config test
```

### About the password

A BMC password has to be stored somewhere if you want unattended monitoring — that is the same
trade-off `ipmitool -f` makes. What `redfishctl` avoids is the ways it commonly leaks: it is never
taken from a command-line argument, so it does not land in your shell history or in a process
listing, and the config file is `0600`.

### About TLS

BMCs ship with self-signed certificates, so verification is **off** by default. Turn it on once
you have installed a real certificate:

```sh
redfishctl config set --verify-tls true
```

## Commands

| | |
|---|---|
| `redfishctl` | The interactive interface |
| `redfishctl status` | Power state, model, consumption, hottest sensor, fan health |
| `redfishctl sensors` | Every sensor with a reading |
| `redfishctl sensors --fans` | Fans only. Also `--temps`, `--volts` |
| `redfishctl sensors --all` | Include sensors the BMC reports as absent |
| `redfishctl power status` | `On` or `Off` |
| `redfishctl power on` | Turn the host on |
| `redfishctl power off` | Ask the OS to shut down cleanly |
| `redfishctl power force-off` | Cut power without asking the OS |
| `redfishctl power reset` | Power cycle without asking the OS |
| `redfishctl watch` | Consumption and hottest sensor, live |
| `redfishctl watch --csv > log.csv` | Same, as CSV with epoch timestamps |
| `redfishctl raw <path>` | GET any Redfish path and print the JSON |


## The interactive interface

| tab | what it is for |
|---|---|
| **Overview** | Is it fine? Machine identity, consumption, and anything wrong in its own panel |
| **Sensors** | Every sensor, filterable, with its limit and state |
| **Charts** | Power over time, plus any sensor you pick |
| **Power** | On, shutdown, force-off, reset — each asks first |
| **Logs** | Event log entries, severity-coloured |

Faults are pulled out into their own panel on the Overview rather than being a colour buried in
a list of ninety, because the question that tab answers is "is it fine?".

Press `e` on a fan or a temperature to edit its critical threshold. Plenty of firmwares expose
those read-only; when yours does, it says so and nothing changes.

### Sensors

Absent sensors are hidden by default. On a board with 92 sensors, two thirds of them report
nothing while the host is powered off, and listing them buries the ones that matter.

Two things get flagged that a plain health field would not:

- a **fan below its critical floor** but still turning — a fan on its way out, worth catching
  before it stops, because one stopped fan puts every other fan in the chassis to maximum;
- a **temperature within 10% of its critical threshold** — warning while there is still time to
  react, rather than reporting that everything was fine right up to the shutdown.

### Watching consumption

```sh
redfishctl watch --interval 1 --csv > run.csv
```

Each row carries a Unix timestamp, so the log can be lined up against whatever you were running
on the machine. That is the point: it turns "the benchmark took 40 seconds" into "the benchmark
cost 300 W for 40 seconds".


## Polling

The interface polls on a timer, and the timing is driven by how slow BMCs are
rather than by taste. Measured on an AMI MegaRAC serving 42 fans:

| | |
|---|---|
| `Chassis/Self/Power` | 1.6 s, 10 KB |
| `Systems/Self` | 2.2 s, 5 KB |
| `Chassis/Self/Thermal` | 2.5 s, 37 KB |
| TLS handshake | 0.38 s, or 0.00002 s on a reused connection |

Two consequences are baked in. The connection is kept alive, which removes the
handshake from every request after the first. And the requests go **one at a
time**: firing all three at once was tried and measured, and it was worse —
6.5 s against 5.2 s, with individual requests stretching to 6.5 s and some
failing outright, because the BMC serialises internally anyway.

Power is polled every interval because it is the number that moves. The other
two are polled one time in four, which takes the common poll to about a second
and a half. Raise `--interval` if your BMC is slower still.

## Compatibility

Written against a Gigabyte board with an AMI MegaRAC BMC (Redfish 1.8). It uses only standard
schema paths, so it should work on any Redfish 1.x implementation. If your BMC names its
resources differently, `redfishctl raw /redfish/v1/Chassis` and `redfishctl raw /redfish/v1/Systems` show
the real ids.

Every field is optional on the way in, so a BMC that implements less of the standard shows less
rather than failing.

## License

MIT
