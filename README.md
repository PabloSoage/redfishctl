# bmctl

Control and monitor a server's BMC from the terminal, over Redfish.

Power the machine on and off, read every sensor it exposes, and follow consumption live —
without opening the BMC's web interface and without `ipmitool` installed.

```
$ bmctl status

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
cargo install --path .
```

Or grab a binary from the releases page.

## Setup

```sh
bmctl config set --host 192.168.1.135 --user admin
```

It prompts for the password without echoing it, and writes everything to a config file
(`bmctl config path` tells you where) with owner-only permissions on Unix.

Then check it:

```sh
bmctl config test
```

### About the password

A BMC password has to be stored somewhere if you want unattended monitoring — that is the same
trade-off `ipmitool -f` makes. What `bmctl` avoids is the ways it commonly leaks: it is never
taken from a command-line argument, so it does not land in your shell history or in a process
listing, and the config file is `0600`.

### About TLS

BMCs ship with self-signed certificates, so verification is **off** by default. Turn it on once
you have installed a real certificate:

```sh
bmctl config set --verify-tls true
```

## Commands

| | |
|---|---|
| `bmctl status` | Power state, model, consumption, hottest sensor, fan health |
| `bmctl sensors` | Every sensor with a reading |
| `bmctl sensors --fans` | Fans only. Also `--temps`, `--volts` |
| `bmctl sensors --all` | Include sensors the BMC reports as absent |
| `bmctl power status` | `On` or `Off` |
| `bmctl power on` | Turn the host on |
| `bmctl power off` | Ask the OS to shut down cleanly |
| `bmctl power force-off` | Cut power without asking the OS |
| `bmctl power reset` | Power cycle without asking the OS |
| `bmctl watch` | Consumption and hottest sensor, live |
| `bmctl watch --csv > log.csv` | Same, as CSV with epoch timestamps |
| `bmctl raw <path>` | GET any Redfish path and print the JSON |

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
bmctl watch --interval 1 --csv > run.csv
```

Each row carries a Unix timestamp, so the log can be lined up against whatever you were running
on the machine. That is the point: it turns "the benchmark took 40 seconds" into "the benchmark
cost 300 W for 40 seconds".

## Compatibility

Written against a Gigabyte board with an AMI MegaRAC BMC (Redfish 1.8). It uses only standard
schema paths, so it should work on any Redfish 1.x implementation. If your BMC names its
resources differently, `bmctl raw /redfish/v1/Chassis` and `bmctl raw /redfish/v1/Systems` show
the real ids.

Every field is optional on the way in, so a BMC that implements less of the standard shows less
rather than failing.

## License

MIT
