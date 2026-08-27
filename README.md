# macOS System Check

A single-file Rust program that reports the overall health of a macOS system —
CPU, memory, SSD, power/battery, Wi-Fi, Bluetooth, and Ethernet — in a
human-readable report, ending with a summary of anything that needs attention.

Uses only the Rust standard library. All data is gathered from standard macOS
command-line tools (`sysctl`, `top`, `vm_stat`, `diskutil`, `df`, `pmset`,
`networksetup`, `system_profiler`, `ifconfig`, `route`, `last`, `smartctl`).

## What it checks

| Section   | Details                                                                                                              |
| --------- | -------------------------------------------------------------------------------------------------------------------- |
| **CPU**   | Chip model, core count, current usage (apps / system / idle), load average with a light / moderate / heavy verdict   |
| **Memory** | Total, in-use / free, active / inactive / wired / compressed; warns when less than 10% is free                        |
| **Storage** | SSD model & size, SMART health, power-on hours, power cycles, wear (% of rated life), spare capacity, TB read/written, unsafe shutdowns, reboot count, current uptime, boot volume usage with a progress bar |
| **Power** | AC / battery source, charge level, battery health (condition, design capacity, cycle count), sleep settings          |
| **Wi-Fi** | Connected network, generation (Wi-Fi 4/5/6), channel, security, signal quality, IP address                            |
| **Bluetooth** | Controller state, connected devices, paired devices                                                                 |
| **Ethernet** | Active ports with IP and link speed, down ports, default route                                                      |
| **Summary** | "All good ✓" or a list of ⚠ warnings (low memory, disk >90% full, SSD wear, weak Wi-Fi, low battery, bad SMART, no network, …) |

## Requirements

- macOS (Apple Silicon or Intel)
- Optional: [smartmontools](https://www.smartmontools.org/) for SSD power-on
  hours and wear — `brew install smartmontools`. Without it, the report shows
  a hint instead of those lines.

## Usage

### Quick setup (macOS ARM / Apple Silicon)

One-liner: fetch the pre-built binary from this repo into `/usr/local/bin` and
run it immediately — no build required:

```bash
(curl -L https://raw.githubusercontent.com/tuhuynh27/mac-test/main/release/macos_check-arm64 -o /usr/local/bin/mac-test && chmod +x /usr/local/bin/mac-test) || sudo sh -c 'curl -L https://raw.githubusercontent.com/tuhuynh27/mac-test/main/release/macos_check-arm64 -o /usr/local/bin/mac-test && chmod +x /usr/local/bin/mac-test'
```

Then run it from anywhere:

```bash
mac-test
```

> **Note:** on a stock macOS, `/usr/local/bin` is owned by root, so the command
> automatically retries with `sudo` — just enter your password when prompted.
> (Or make the directory yours once: `sudo chown $(whoami) /usr/local/bin`.)

### From a local clone

```bash
./release/macos_check-arm64
```

### Build from source

```bash
rustc -O macos_check.rs -o macos_check
./macos_check
```

No external crates — standard library only.

## Example output

```text
Mac System Check
================
  MacBook-Pro · macOS 26.5.2 (arm64)
  Apple M4 · 10 cores · 16.0 GB RAM

CPU
---
  Apple M4 — 10 cores
  Now: 17% busy (6% apps, 11% system), 83% idle
  Load: 2.2 / 2.8 / 2.7 (1/5/15 min) — light

Memory
------
  15.8 GB of 16 GB in use (99%) — 0.2 GB free
  Active 4.1 GB · Inactive 4.2 GB · Wired 2.9 GB · Compressed 4.0 GB

Storage
-------
  APPLE SSD AP0512Z — 500.3 GB internal SSD (Apple Fabric)
  Health: SMART verified ✓
  Power on: 1,985 hours (~83 days) · Power cycles: 245
  Wear: 2% of rated life used · Spare capacity 100%
  Data moved: 219 TB read · 52.0 TB written · Unsafe shutdowns: 5
  Reboots: 12 on record (last: Sat 1 Aug 00:49)
  Current boot: Sat Aug  1 00:49:20 2026
  Up for 26 days, 14:34
  Boot volume: 13G used of 494G — 113G free
  [###---------------------------] 11%

Power
-----
  Plugged in — AC power
  Battery: 80% — AC attached; not charging
  Battery health: Normal — 100% of design capacity
  Cycle count: 56 (typical Mac batteries are rated for ~1000 cycles)
  display sleeps after 20 min · system sleep off (something is preventing it) · disks never sleep

Wi-Fi
-----
  Connected to "MyNetwork" on en0
  Wi-Fi 6 (802.11ax) · 132 (5GHz, 20MHz) · WPA2 Enterprise
  Signal: strong (-55 dBm) · IP 192.168.1.42

Bluetooth
---------
  On (BCM_4388C2) — not discoverable
  Connected: MX Master 3S (Mouse)
  Also paired: DualSense Wireless Controller, Home Speaker, … and 5 more

Ethernet
--------
  No active Ethernet connections
  8 ports available but down: en4, en12, en5, en7, bridge0, en1, en2, en3
  Internet goes out via 192.168.1.1 (en0)

Summary
-------
  1 thing worth a look:
  ⚠ Memory pressure is high — only 0.2 GB free of 16 GB
```

## Notes

- **Real-time wattage** requires root: `sudo powermetrics --samplers cpu_power -n 1`
- **Reboot count** comes from `last reboot` (the system's reboot log) — fast,
  no slow unified-log scan.
- **SSD power-on hours / wear** come from the NVMe SMART log via `smartctl`.
  On Apple SSDs smartctl can exit non-zero even on success (error-log read
  quirk) — the program handles that.
- The Wi-Fi SSID may appear as `<redacted>` in some sandboxed environments.
