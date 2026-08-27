//! macOS system health checker — human-readable report
//!
//! Checks: CPU, memory, SSD, power, Wi-Fi, Bluetooth, Ethernet,
//! and ends with a short summary of anything worth attention.
//!
//! Uses only the Rust standard library; data comes from standard
//! macOS tools (sysctl, top, vm_stat, diskutil, df, pmset,
//! networksetup, system_profiler, ifconfig, route).
//!
//! Build & run:
//!   rustc -O macos_check.rs && ./macos_check

use std::process::Command;

// ---------------------------------------------------------------- helpers

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Like `run`, but keeps the output even on a non-zero exit status.
/// (e.g. smartctl exits 4 on Apple SSDs even when it read the SMART data fine.)
fn run_loose(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Value of the first line starting with `prefix` (e.g. "SMART Status:").
fn field(output: &str, prefix: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let t = line.trim();
        t.strip_prefix(prefix)
            .map(|rest| rest.trim().trim_start_matches(':').trim().to_string())
    })
}

fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    if g >= 100.0 {
        format!("{} GB", g.round() as u64)
    } else {
        format!("{:.1} GB", g)
    }
}

/// Parse a number that may contain thousands separators ("1,985" -> 1985.0).
fn parse_num(s: &str) -> f64 {
    s.replace(',', "").trim().parse().unwrap_or(0.0)
}

/// "428,083,309 [219 TB]" -> "219 TB" (prefer the human-friendly bracketed part).
fn tb_value(s: &str) -> String {
    if let Some(start) = s.find('[') {
        if let Some(end) = s[start..].find(']') {
            return s[start + 1..start + end].trim().to_string();
        }
    }
    s.to_string()
}

fn hr(title: &str) {
    println!();
    println!("{}", title);
    println!("{}", "-".repeat(title.len()));
}

/// Small progress bar: "[###-----------------------------] 11%"
fn bar(pct: f64) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let width = 30;
    let filled = (pct / 100.0 * width as f64).round() as usize;
    let mut s = String::from("[");
    for i in 0..width {
        s.push(if i < filled { '#' } else { '-' });
    }
    format!("{}] {}%", s, pct.round() as u64)
}

fn signal_quality(dbm: i32) -> &'static str {
    match dbm {
        n if n >= -50 => "excellent",
        n if n >= -60 => "strong",
        n if n >= -70 => "fair",
        n if n >= -80 => "weak",
        _ => "very weak",
    }
}

fn wifi_generation(phy: &str) -> &'static str {
    if phy.contains("ax") {
        "Wi-Fi 6"
    } else if phy.contains("ac") {
        "Wi-Fi 5"
    } else if phy.contains('n') {
        "Wi-Fi 4"
    } else if phy.contains('g') {
        "Wi-Fi 3"
    } else if phy.contains('b') {
        "Wi-Fi 2"
    } else {
        "802.11"
    }
}

fn load_word(load: f64, cores: f64) -> &'static str {
    if cores <= 0.0 {
        return "n/a";
    }
    match load / cores {
        r if r < 0.5 => "light",
        r if r < 1.0 => "moderate",
        r if r < 2.0 => "heavy",
        _ => "overloaded",
    }
}

/// Shared state so the summary can cross-check sections.
struct Report {
    warnings: Vec<String>,
    wifi_connected: bool,
    eth_active: bool,
}

// ------------------------------------------------------------------ CPU

fn cpu(chip: &str, cores: u64, top: Option<&str>, r: &mut Report) {
    hr("CPU");
    println!("  {} — {} cores", chip, cores);
    if let Some(top) = top {
        if let Some(line) = top.lines().find(|l| l.contains("CPU usage:")) {
            let nums: Vec<f64> = line
                .split(',')
                .filter_map(|p| {
                    p.split('%')
                        .next()
                        .and_then(|s| s.split_whitespace().last())
                        .and_then(|s| s.parse().ok())
                })
                .collect();
            if nums.len() == 3 {
                let (user, sys, idle) = (nums[0], nums[1], nums[2]);
                println!(
                    "  Now: {}% busy ({}% apps, {}% system), {}% idle",
                    (100.0 - idle).round(),
                    user.round(),
                    sys.round(),
                    idle.round()
                );
            }
        }
        if let Some(line) = top.lines().find(|l| l.contains("Load Avg:")) {
            let nums: Vec<f64> = line
                .split(':')
                .nth(1)
                .unwrap_or("")
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if nums.len() == 3 {
                let word = load_word(nums[0], cores as f64);
                println!(
                    "  Load: {:.1} / {:.1} / {:.1} (1/5/15 min) — {}",
                    nums[0], nums[1], nums[2], word
                );
                if word == "heavy" || word == "overloaded" {
                    r.warnings
                        .push(format!("CPU load is {} ({:.1} on {} cores)", word, nums[0], cores));
                }
            }
        }
    }
}

// ---------------------------------------------------------------- Memory

fn memory(total_bytes: u64, r: &mut Report) {
    hr("Memory");
    if total_bytes == 0 {
        println!("  (unknown)");
        return;
    }
    let total = total_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let mut stats = [0.0f64; 5]; // active, inactive, wired, free, compressed
    if let Some(vm) = run("vm_stat", &[]) {
        let page = vm
            .lines()
            .find_map(|l| l.trim().strip_prefix("page size of "))
            .and_then(|s| s.trim().trim_end_matches(" bytes.").parse::<f64>().ok())
            .unwrap_or(16384.0);
        let pages = |key: &str| -> f64 {
            vm.lines()
                .find_map(|l| {
                    l.trim().strip_prefix(key).map(|rest| {
                        rest.trim()
                            .trim_start_matches(':')
                            .trim()
                            .trim_end_matches('.')
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    })
                })
                .unwrap_or(0.0)
                * page
                / 1024.0
                / 1024.0
                / 1024.0
        };
        stats[0] = pages("Pages active");
        stats[1] = pages("Pages inactive");
        stats[2] = pages("Pages wired down");
        stats[3] = pages("Pages free");
        stats[4] = pages("Pages occupied by compressor");
    }
    let (active, inactive, wired, free, compressed) =
        (stats[0], stats[1], stats[2], stats[3], stats[4]);
    let used = (total - free).max(0.0);
    let pct = if total > 0.0 { used / total * 100.0 } else { 0.0 };
    println!(
        "  {:.1} GB of {:.0} GB in use ({}%) — {:.1} GB free",
        used,
        total,
        pct.round(),
        free
    );
    println!(
        "  Active {:.1} GB · Inactive {:.1} GB · Wired {:.1} GB · Compressed {:.1} GB",
        active, inactive, wired, compressed
    );
    if free / total < 0.10 {
        r.warnings.push(format!(
            "Memory pressure is high — only {:.1} GB free of {:.0} GB",
            free, total
        ));
    }
}

// ---------------------------------------------------------------- Storage

fn storage(r: &mut Report) {
    hr("Storage");
    let info = run("diskutil", &["info", "disk0"]).unwrap_or_default();
    let name = field(&info, "Device / Media Name:").unwrap_or_else(|| "disk0".into());
    let size = field(&info, "Disk Size:")
        .map(|s| s.split('(').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    let smart = field(&info, "SMART Status:").unwrap_or_default();
    let is_ssd = field(&info, "Solid State:").unwrap_or_default() == "Yes";
    let proto = field(&info, "Protocol:").unwrap_or_default();

    println!(
        "  {} — {} internal {} ({})",
        name,
        size,
        if is_ssd { "SSD" } else { "disk" },
        proto
    );
    if smart == "Verified" {
        println!("  Health: SMART verified ✓");
    } else if !smart.is_empty() {
        println!("  Health: {}", smart);
        r.warnings.push(format!("SSD SMART status is '{}'", smart));
    }

    // Usage, wear & reboot history (needs smartmontools: brew install smartmontools)
    match run_loose("smartctl", &["-a", "/dev/disk0"]) {
        Some(out) if out.contains("Power On Hours") => {
            let poh = field(&out, "Power On Hours:").unwrap_or_default();
            let cycles = field(&out, "Power Cycles:").unwrap_or_default();
            let used = field(&out, "Percentage Used:").unwrap_or_default();
            let spare = field(&out, "Available Spare:").unwrap_or_default();
            let read = field(&out, "Data Units Read:").as_deref().map(tb_value).unwrap_or_default();
            let written = field(&out, "Data Units Written:").as_deref().map(tb_value).unwrap_or_default();
            let unsafe_sh = field(&out, "Unsafe Shutdowns:").unwrap_or_default();

            println!(
                "  Power on: {} hours (~{} days) · Power cycles: {}",
                poh,
                (parse_num(&poh) / 24.0).round() as u64,
                cycles
            );
            println!("  Wear: {} of rated life used · Spare capacity {}", used, spare);
            println!(
                "  Data moved: {} read · {} written · Unsafe shutdowns: {}",
                read, written, unsafe_sh
            );
            if let Some(pct) = used.trim_end_matches('%').parse::<f64>().ok() {
                if pct >= 90.0 {
                    r.warnings.push(format!(
                        "SSD has used {}% of its rated life",
                        pct.round() as u64
                    ));
                }
            }
        }
        _ => {
            println!("  (Power-on hours & wear: install smartmontools — brew install smartmontools)");
        }
    }

    // Reboot history (fast — reads the system's reboot log, not the unified log)
    let last = run("last", &["reboot"]).unwrap_or_default();
    let reboots: Vec<&str> = last.lines().filter(|l| l.contains("reboot time")).collect();
    if !reboots.is_empty() {
        let last_reboot = reboots[0]
            .trim()
            .split_whitespace()
            .skip(2)
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        println!("  Reboots: {} on record (last: {})", reboots.len(), last_reboot);
    }
    if let Some(bt) = run("sysctl", &["-n", "kern.boottime"]) {
        if let Some(human) = bt.split('}').nth(1) {
            println!("  Current boot: {}", human.trim());
        }
    }
    if let Some(up) = run("uptime", &[]) {
        if let Some(part) = up.split("up ").nth(1) {
            let part = part.split(',').take(2).collect::<Vec<_>>().join(",");
            println!("  Up for {}", part.trim());
        }
    }

    if let Some(df) = run("df", &["-H", "/"]) {
        let cols: Vec<&str> = df.lines().nth(1).unwrap_or("").split_whitespace().collect();
        if cols.len() >= 5 {
            let pct: f64 = cols[4].trim_end_matches('%').parse().unwrap_or(0.0);
            println!(
                "  Boot volume: {} used of {} — {} free",
                cols[2], cols[1], cols[3]
            );
            println!("  {}", bar(pct));
            if pct >= 90.0 {
                r.warnings.push(format!("Boot volume is {} full", cols[4]));
            }
        }
    }
}

// ----------------------------------------------------------------- Power

fn power(r: &mut Report) {
    hr("Power");
    let batt = run("pmset", &["-g", "batt"]).unwrap_or_default();
    let source = batt
        .lines()
        .find_map(|l| l.trim().strip_prefix("Now drawing from '"))
        .map(|s| s.trim_end_matches('\'').to_string());

    match source.as_deref() {
        Some("AC Power") => println!("  Plugged in — AC power"),
        Some("Battery Power") => println!("  Running on battery"),
        _ => {}
    }

    match batt.lines().find(|l| l.contains("InternalBattery")) {
        Some(line) => {
            let parts: Vec<&str> = line.split(';').collect();
            let pct: f64 = parts[0]
                .split_whitespace()
                .last()
                .and_then(|s| s.trim_end_matches('%').parse().ok())
                .unwrap_or(0.0);
            let state = parts[1..]
                .iter()
                .map(|s| s.trim().replace("present: true", "").replace("present: false", ""))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            println!("  Battery: {}% — {}", pct.round() as u64, state);
            if pct < 20.0 && source.as_deref() != Some("AC Power") {
                r.warnings.push(format!(
                    "Battery is low ({}%) and not plugged in",
                    pct.round() as u64
                ));
            }
        }
        None => println!("  No battery (desktop Mac)"),
    }

    // Battery health & cycle count
    if let Some(prof) = run("system_profiler", &["SPPowerDataType"]) {
        if let (Some(cycles), Some(condition)) =
            (field(&prof, "Cycle Count:"), field(&prof, "Condition:"))
        {
            let cap_part = field(&prof, "Maximum Capacity:")
                .map(|c| format!(" — {} of design capacity", c))
                .unwrap_or_default();
            println!("  Battery health: {}{}", condition, cap_part);
            println!(
                "  Cycle count: {} (typical Mac batteries are rated for ~1000 cycles)",
                cycles
            );
            if condition != "Normal" {
                r.warnings.push(format!("Battery condition is '{}'", condition));
            }
            if parse_num(&cycles) >= 1000.0 {
                r.warnings.push(format!(
                    "Battery has {} cycles — beyond the typical ~1000-cycle rating",
                    cycles
                ));
            }
            if let Some(mc) = field(&prof, "Maximum Capacity:") {
                if let Some(pct) = mc.trim_end_matches('%').parse::<f64>().ok() {
                    if pct < 80.0 {
                        r.warnings.push(format!(
                            "Battery holds only {}% of its design capacity",
                            pct.round() as u64
                        ));
                    }
                }
            }
        }
    }

    let g = run("pmset", &["-g"]).unwrap_or_default();
    let mut parts = Vec::new();
    if let Some(v) = field(&g, "displaysleep") {
        parts.push(if v == "0" {
            "display never sleeps".into()
        } else {
            format!("display sleeps after {} min", v)
        });
    }
    if let Some(v) = field(&g, "sleep") {
        let prevented = v.contains("prevented");
        let num = v.split_whitespace().next().unwrap_or("0");
        parts.push(if num == "0" {
            if prevented {
                "system sleep off (something is preventing it)".into()
            } else {
                "system sleep off".into()
            }
        } else {
            format!("system sleeps after {} min", num)
        });
    }
    if let Some(v) = field(&g, "disksleep") {
        parts.push(if v == "0" {
            "disks never sleep".into()
        } else {
            format!("disks sleep after {} min", v)
        });
    }
    if !parts.is_empty() {
        println!("  {}", parts.join(" · "));
    }
    println!("  (For real-time wattage: sudo powermetrics --samplers cpu_power -n 1)");
}

// ----------------------------------------------------------------- Wi-Fi

/// (ssid, phy mode, channel, security, signal dBm) from the
/// "Current Network Information:" block of SPAirPortDataType.
fn wifi_details(prof: &str) -> Option<(String, String, String, String, i32)> {
    let lines: Vec<&str> = prof.lines().collect();
    let start = lines.iter().position(|l| l.trim() == "Current Network Information:")?;
    let ssid = lines.get(start + 1)?.trim().trim_end_matches(':').to_string();
    let (mut phy, mut chan, mut sec, mut sig) =
        (String::new(), String::new(), String::new(), 0i32);
    for l in lines.iter().skip(start + 2) {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("Other Local") || t.starts_with("Interfaces:") || t.starts_with("Wi-Fi:") {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            match k.trim() {
                "PHY Mode" => phy = v.trim().to_string(),
                "Channel" => chan = v.trim().to_string(),
                "Security" => sec = v.trim().to_string(),
                "Signal / Noise" => {
                    sig = v
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                }
                _ => {}
            }
        }
    }
    Some((ssid, phy, chan, sec, sig))
}

fn wifi(ports: &str, r: &mut Report) {
    hr("Wi-Fi");
    let mut iface = String::new();
    let mut want = false;
    for line in ports.lines() {
        let t = line.trim();
        if t.starts_with("Hardware Port:") {
            want = t.contains("Wi-Fi");
        } else if want && t.starts_with("Device: ") {
            iface = t.trim_start_matches("Device: ").to_string();
            break;
        }
    }
    if iface.is_empty() {
        iface = "en0".into();
    }

    let prof = run("system_profiler", &["SPAirPortDataType"]).unwrap_or_default();
    match wifi_details(&prof) {
        Some((ssid, phy, chan, sec, sig)) => {
            r.wifi_connected = true;
            println!("  Connected to \"{}\" on {}", ssid, iface);
            println!(
                "  {} ({}) · {} · {}",
                wifi_generation(&phy),
                phy,
                chan,
                sec
            );
            let quality = signal_quality(sig);
            let ip = run("ifconfig", &[&iface])
                .and_then(|ifc| {
                    ifc.lines().find_map(|l| {
                        l.trim()
                            .strip_prefix("inet ")
                            .and_then(|s| s.split_whitespace().next())
                            .map(|s| s.to_string())
                    })
                })
                .unwrap_or_default();
            println!(
                "  Signal: {} ({} dBm) · IP {}",
                quality,
                sig,
                if ip.is_empty() { "n/a" } else { &ip }
            );
            if sig < -70 {
                r.warnings
                    .push(format!("Wi-Fi signal is {} ({} dBm)", quality, sig));
            }
        }
        None => println!("  Not connected to any network"),
    }
}

// ------------------------------------------------------------- Bluetooth

fn bluetooth(r: &mut Report) {
    hr("Bluetooth");
    let prof = match run("system_profiler", &["SPBluetoothDataType"]) {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            println!("  Not available on this Mac");
            return;
        }
    };

    let mut section = String::new();
    let mut dev = String::new();
    let mut state = String::new();
    let mut chipset = String::new();
    let mut discoverable = String::new();
    let mut connected: Vec<(String, String)> = Vec::new(); // (name, type)
    let mut known: Vec<String> = Vec::new();

    for line in prof.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= 6 && t.ends_with(':') {
            section = match t {
                "Bluetooth Controller:" => "controller",
                "Connected:" => "connected",
                "Not Connected:" => "known",
                _ => "",
            }
            .to_string();
            dev.clear();
            continue;
        }
        if section.is_empty() {
            continue;
        }
        if t.ends_with(':') {
            dev = t.trim_end_matches(':').to_string();
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            match section.as_str() {
                "controller" => match k {
                    "State" => state = v.to_string(),
                    "Chipset" => chipset = v.to_string(),
                    "Discoverable" => discoverable = v.to_string(),
                    _ => {}
                },
                "connected" if !dev.is_empty() => match k {
                    "Address" => connected.push((dev.clone(), String::new())),
                    "Minor Type" => {
                        if let Some(e) = connected.last_mut() {
                            e.1 = v.to_string();
                        }
                    }
                    _ => {}
                },
                "known" if !dev.is_empty() && k == "Address" => known.push(dev.clone()),
                _ => {}
            }
        }
    }

    match state.as_str() {
        "On" => println!(
            "  On ({}) — {}",
            chipset,
            if discoverable == "On" { "discoverable" } else { "not discoverable" }
        ),
        other => {
            println!("  {}", other);
            r.warnings.push(format!("Bluetooth is {}", other));
        }
    }
    if connected.is_empty() {
        println!("  No devices connected");
    } else {
        for (name, dtype) in &connected {
            println!(
                "  Connected: {} ({})",
                name,
                if dtype.is_empty() { "?" } else { dtype }
            );
        }
    }
    if !known.is_empty() {
        let shown: Vec<&str> = known.iter().take(6).map(|s| s.as_str()).collect();
        let extra = known.len().saturating_sub(6);
        let suffix = if extra > 0 {
            format!(" … and {} more", extra)
        } else {
            String::new()
        };
        println!("  Also paired: {}{}", shown.join(", "), suffix);
    }
}

// ---------------------------------------------------------------- Ethernet

/// "Maximum Link Speed" from the SPEthernetDataType block whose
/// "BSD Device Name" matches `dev`.
fn max_link_speed(prof: &str, dev: &str) -> Option<String> {
    let lines: Vec<&str> = prof.lines().collect();
    let mut in_block = false;
    for l in lines {
        let t = l.trim();
        if t.starts_with("BSD Device Name:") {
            in_block = t.trim_start_matches("BSD Device Name:").trim() == dev;
            continue;
        }
        if in_block {
            if let Some(v) = t.strip_prefix("Maximum Link Speed:") {
                return Some(v.trim().to_string());
            }
            if l.starts_with("    ") && !l.starts_with("      ") && t.ends_with(':') {
                in_block = false;
            }
        }
    }
    None
}

fn ethernet(ports: &str, r: &mut Report) {
    hr("Ethernet");
    let mut devices: Vec<(String, String)> = Vec::new();
    let mut name = String::new();
    for line in ports.lines() {
        let t = line.trim();
        if t.starts_with("Hardware Port:") {
            name = t.trim_start_matches("Hardware Port: ").to_string();
        } else if t.starts_with("Device: ") {
            let dev = t.trim_start_matches("Device: ").to_string();
            if !name.contains("Wi-Fi") {
                devices.push((name.clone(), dev));
            }
        }
    }

    if devices.is_empty() {
        println!("  No Ethernet hardware ports found");
        return;
    }

    let prof = run("system_profiler", &["SPEthernetDataType"]).unwrap_or_default();
    let mut down: Vec<String> = Vec::new();
    for (name, dev) in &devices {
        let ifc = run("ifconfig", &[dev]).unwrap_or_default();
        if ifc.lines().any(|l| l.trim() == "status: active") {
            r.eth_active = true;
            let ip = ifc
                .lines()
                .find_map(|l| {
                    l.trim()
                        .strip_prefix("inet ")
                        .map(|s| s.split_whitespace().next().unwrap_or(""))
                })
                .unwrap_or("n/a");
            let speed = max_link_speed(&prof, dev).unwrap_or_default();
            let speed_part = if speed.is_empty() {
                String::new()
            } else {
                format!(", up to {}", speed)
            };
            println!("  {} ({}) — active, IP {}{}", name, dev, ip, speed_part);
        } else {
            down.push(dev.clone());
        }
    }
    if !r.eth_active {
        println!("  No active Ethernet connections");
        if !down.is_empty() {
            println!(
                "  {} ports available but down: {}",
                down.len(),
                down.join(", ")
            );
        }
    }
    if let Some(route) = run("route", &["-n", "get", "default"]) {
        if let Some(gw) = field(&route, "gateway") {
            let iface = field(&route, "interface").unwrap_or_default();
            println!("  Internet goes out via {} ({})", gw, iface);
        }
    }
}

// ---------------------------------------------------------------- Summary

fn summary(r: &Report) {
    hr("Summary");
    let mut warnings = r.warnings.clone();
    if !r.wifi_connected && !r.eth_active {
        warnings
            .push("No network connection (Wi-Fi not associated, no active Ethernet)".into());
    }
    if warnings.is_empty() {
        println!("  All good ✓ — nothing needs attention.");
    } else {
        println!(
            "  {} thing{} worth a look:",
            warnings.len(),
            if warnings.len() == 1 { "" } else { "s" }
        );
        for w in &warnings {
            println!("  ⚠ {}", w);
        }
    }
    println!();
}

// ------------------------------------------------------------------ main

fn main() {
    let host = run("hostname", &[]).unwrap_or_default().trim().to_string();
    let os = run("sw_vers", &["-productVersion"]).unwrap_or_default().trim().to_string();
    let arch = run("uname", &["-m"]).unwrap_or_default().trim().to_string();
    let model = run("sysctl", &["-n", "hw.model"]).unwrap_or_default().trim().to_string();
    let brand = run("sysctl", &["-n", "machdep.cpu.brand_string"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let chip = if brand.is_empty() { model } else { brand };
    let cores: u64 = run("sysctl", &["-n", "hw.logicalcpu"])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let ram_bytes: u64 = run("sysctl", &["-n", "hw.memsize"])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    println!("Mac System Check");
    println!("================");
    println!("  {} · macOS {} ({})", host, os, arch);
    println!("  {} · {} cores · {} RAM", chip, cores, gb(ram_bytes));

    let top = run("top", &["-l", "1", "-n", "0"]);
    let ports = run("networksetup", &["-listallhardwareports"]).unwrap_or_default();

    let mut r = Report {
        warnings: Vec::new(),
        wifi_connected: false,
        eth_active: false,
    };
    cpu(&chip, cores, top.as_deref(), &mut r);
    memory(ram_bytes, &mut r);
    storage(&mut r);
    power(&mut r);
    wifi(&ports, &mut r);
    bluetooth(&mut r);
    ethernet(&ports, &mut r);
    summary(&r);
}
