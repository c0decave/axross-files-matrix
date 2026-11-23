//! The machine as Linux shows it without root: `/proc` (the processor, the
//! memory, the load), `/sys` (the block devices and their partitions, PCI,
//! network and USB devices, the board's DMI strings, the memory modules
//! where EDAC or the SMBIOS tables are readable). Every reader takes the
//! root it reads below (`/` here, a fixture folder in the tests); whatever
//! cannot be read is left empty - nothing here fails, and nothing runs a
//! program.

use super::*;
use std::fs;
use std::path::Path;

fn read(root: &Path, rel: &str) -> Option<String> {
    let s = fs::read_to_string(root.join(rel.trim_start_matches('/'))).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.chars().take(200).collect())
}

fn read_u64(root: &Path, rel: &str) -> Option<u64> {
    read(root, rel)?.split_whitespace().next()?.parse().ok()
}

fn list(root: &Path, rel: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root.join(rel.trim_start_matches('/')))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| natural(a, b));
    v
}

/// "cpu10" after "cpu9".
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let i = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
        (
            s[..i].to_string(),
            s[i..].parse::<u64>().ok(),
            s.to_string(),
        )
    };
    split(a).cmp(&split(b))
}

/// Everything there is to read below `root`.
pub fn detect(root: &Path) -> Machine {
    Machine {
        host: read(root, "/proc/sys/kernel/hostname").unwrap_or_default(),
        os: os_name(root),
        kernel: read(root, "/proc/sys/kernel/osrelease").unwrap_or_default(),
        board: board(root),
        cpu: cpu(root),
        memory: memory(root),
        disks: disks(root),
        pci: pci(root),
        nics: nics(root),
        usb: usb(root),
    }
}

fn os_name(root: &Path) -> String {
    let text = read(root, "/etc/os-release").unwrap_or_default();
    let full = fs::read_to_string(root.join("etc/os-release")).unwrap_or(text);
    full.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').chars().take(80).collect())
        .unwrap_or_default()
}

fn board(root: &Path) -> Board {
    let dmi = |f: &str| read(root, &format!("/sys/class/dmi/id/{f}")).filter(|v| !placeholder(v));
    Board {
        vendor: dmi("board_vendor"),
        name: dmi("board_name"),
        version: dmi("board_version"),
        system_vendor: dmi("sys_vendor"),
        product: dmi("product_name"),
        bios_vendor: dmi("bios_vendor"),
        bios_version: dmi("bios_version"),
        bios_date: dmi("bios_date"),
    }
}

/// DMI fields firmware fills with nothing useful.
fn placeholder(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    [
        "to be filled by o.e.m.",
        "default string",
        "not specified",
        "not applicable",
        "system product name",
        "system manufacturer",
        "none",
        "n/a",
        "0123456789",
    ]
    .iter()
    .any(|p| l == *p)
}

pub fn cpu(root: &Path) -> Cpu {
    let text = fs::read_to_string(root.join("proc/cpuinfo")).unwrap_or_default();
    let mut cpu = Cpu::default();
    let mut current: Option<Core> = None;
    let mut packages = std::collections::BTreeSet::new();
    let mut physical = std::collections::BTreeSet::new();
    let flush = |current: &mut Option<Core>, cpu: &mut Cpu| {
        if let Some(c) = current.take() {
            cpu.cores.push(c);
        }
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "processor" => {
                flush(&mut current, &mut cpu);
                current = Some(Core {
                    id: v.parse().unwrap_or(cpu.cores.len() as u32),
                    ..Default::default()
                });
            }
            "model name" | "Model" | "cpu model" if cpu.model.is_empty() => {
                cpu.model = v.chars().take(80).collect()
            }
            "vendor_id" | "CPU implementer" if cpu.vendor.is_empty() => cpu.vendor = v.into(),
            "physical id" => {
                if let Some(c) = current.as_mut() {
                    c.package = v.parse().unwrap_or(0);
                    packages.insert(c.package);
                }
            }
            "core id" => {
                if let Some(c) = current.as_mut() {
                    c.core = v.parse().unwrap_or(0);
                    physical.insert((c.package, c.core));
                }
            }
            "cpu MHz" => {
                if let Some(c) = current.as_mut() {
                    c.mhz = v.parse::<f32>().unwrap_or(0.0) as u32;
                }
            }
            _ => {}
        }
    }
    flush(&mut current, &mut cpu);
    for c in &mut cpu.cores {
        if let Some(khz) = read_u64(
            root,
            &format!(
                "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
                c.id
            ),
        ) {
            c.max_mhz = (khz / 1000) as u32;
        }
    }
    if cpu.cores.is_empty() {
        // (no cpuinfo: as many as there are cpuN folders)
        for name in list(root, "/sys/devices/system/cpu") {
            if let Some(n) = name.strip_prefix("cpu").and_then(|n| n.parse::<u32>().ok()) {
                cpu.cores.push(Core {
                    id: n,
                    ..Default::default()
                });
            }
        }
    }
    cpu.packages = packages.len().max(1) as u32;
    cpu.physical = if physical.is_empty() {
        cpu.cores.len() as u32
    } else {
        physical.len() as u32
    };
    if cpu.model.is_empty() {
        cpu.model = "processor".into();
    }
    cpu
}

pub fn memory(root: &Path) -> Memory {
    let text = fs::read_to_string(root.join("proc/meminfo")).unwrap_or_default();
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map(|v| v * 1024)
    };
    let mut m = Memory {
        total: kb("MemTotal").unwrap_or(0),
        available: kb("MemAvailable").or_else(|| kb("MemFree")).unwrap_or(0),
        swap_total: kb("SwapTotal").unwrap_or(0),
        swap_free: kb("SwapFree").unwrap_or(0),
        ..Default::default()
    };
    // the modules: the SMBIOS tables (type 17) where readable, else EDAC
    let mut modules = Vec::new();
    for e in list(root, "/sys/firmware/dmi/entries") {
        if !e.starts_with("17-") {
            continue;
        }
        if let Ok(raw) = fs::read(root.join(format!("sys/firmware/dmi/entries/{e}/raw")))
            && let Some(module) = smbios_memory_device(&raw)
        {
            modules.push(module);
        }
    }
    if modules.is_empty() {
        for mc in list(root, "/sys/devices/system/edac/mc") {
            if !mc.starts_with("mc") {
                continue;
            }
            for d in list(root, &format!("/sys/devices/system/edac/mc/{mc}")) {
                if !(d.starts_with("dimm") || d.starts_with("rank")) {
                    continue;
                }
                let base = format!("/sys/devices/system/edac/mc/{mc}/{d}");
                let size = read_u64(root, &format!("{base}/size")).unwrap_or(0) << 20;
                modules.push(MemModule {
                    slot: read(root, &format!("{base}/dimm_label")).unwrap_or_else(|| d.clone()),
                    size,
                    kind: read(root, &format!("{base}/dimm_mem_type")).map(|t| {
                        t.trim_start_matches("Unbuffered-")
                            .trim_start_matches("Registered-")
                            .to_string()
                    }),
                    speed: None,
                    maker: None,
                    part: None,
                });
            }
        }
    }
    m.kind = modules.iter().find_map(|x| x.kind.clone());
    m.modules = modules;
    m
}

/// One SMBIOS memory device (type 17) structure: its slot, size, type,
/// speed and maker (an empty slot has size 0).
pub fn smbios_memory_device(raw: &[u8]) -> Option<MemModule> {
    if raw.len() < 0x15 || raw[0] != 17 {
        return None;
    }
    let len = raw[1] as usize;
    if len < 0x15 || len > raw.len() {
        return None;
    }
    let word = |o: usize| (o + 1 < len).then(|| u16::from_le_bytes([raw[o], raw[o + 1]]));
    let dword = |o: usize| {
        (o + 3 < len).then(|| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]))
    };
    // the strings after the formatted area, 1-based
    let strings: Vec<String> = raw[len..]
        .split(|&b| b == 0)
        .take_while(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).trim().chars().take(64).collect())
        .collect();
    let string = |o: usize| -> Option<String> {
        let i = *raw.get(o).filter(|_| o < len)? as usize;
        let s = strings.get(i.checked_sub(1)?)?.clone();
        (!s.is_empty() && !placeholder(&s) && s != "Unknown").then_some(s)
    };
    let size = match word(0x0c)? {
        0 | 0xffff => 0,
        0x7fff => (dword(0x1c).unwrap_or(0) as u64) << 20,
        v if v & 0x8000 != 0 => ((v & 0x7fff) as u64) << 10,
        v => (v as u64) << 20,
    };
    let kind = raw.get(0x12).filter(|_| 0x12 < len).and_then(|t| {
        Some(
            match t {
                0x12 => "DDR",
                0x13 => "DDR2",
                0x18 => "DDR3",
                0x1a => "DDR4",
                0x1b => "LPDDR",
                0x1c => "LPDDR2",
                0x1d => "LPDDR3",
                0x1e => "LPDDR4",
                0x22 => "DDR5",
                0x23 => "LPDDR5",
                _ => return None,
            }
            .to_string(),
        )
    });
    Some(MemModule {
        slot: string(0x10).unwrap_or_else(|| "slot".into()),
        size,
        kind,
        speed: word(0x15)
            .filter(|s| *s != 0 && *s != 0xffff)
            .map(u32::from),
        maker: string(0x17),
        part: string(0x1a),
    })
}

fn dev_of(root: &Path, rel: &str) -> Option<(u32, u32)> {
    let s = read(root, rel)?;
    let (a, b) = s.split_once(':')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

pub fn disks(root: &Path) -> Vec<Disk> {
    let mut out = Vec::new();
    for name in list(root, "/sys/block") {
        if ["loop", "ram", "zram", "fd", "sr"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let base = format!("/sys/block/{name}");
        let sectors = read_u64(root, &format!("{base}/size")).unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let mut partitions = Vec::new();
        for p in list(root, &base) {
            if !p.starts_with(&name) || p == name {
                continue;
            }
            if let Some(dev) = dev_of(root, &format!("{base}/{p}/dev")) {
                partitions.push(Partition {
                    name: p.clone(),
                    dev,
                    size: read_u64(root, &format!("{base}/{p}/size")).unwrap_or(0) * 512,
                });
            }
        }
        // (device mapper: the disks below it - LVM, encryption)
        let below: Vec<String> = list(root, &format!("{base}/slaves"));
        let removable = read_u64(root, &format!("{base}/removable")) == Some(1);
        let transport = if name.starts_with("nvme") {
            "NVMe"
        } else if name.starts_with("mmcblk") {
            "SD / eMMC"
        } else if name.starts_with("vd") || name.starts_with("xvd") {
            "virtual"
        } else if name.starts_with("dm-") || name.starts_with("md") {
            "mapped"
        } else if removable {
            "USB / removable"
        } else {
            "SATA / SCSI"
        };
        out.push(Disk {
            name: name.clone(),
            model: read(root, &format!("{base}/device/model"))
                .or_else(|| read(root, &format!("{base}/dm/name"))),
            vendor: read(root, &format!("{base}/device/vendor")),
            size: sectors * 512,
            rotational: read_u64(root, &format!("{base}/queue/rotational")) == Some(1),
            removable,
            transport: transport.into(),
            dev: dev_of(root, &format!("{base}/dev")).unwrap_or((0, 0)),
            partitions,
            below,
        });
    }
    out
}

pub fn pci(root: &Path) -> Vec<PciDevice> {
    let mut out = Vec::new();
    for slot in list(root, "/sys/bus/pci/devices") {
        let base = format!("/sys/bus/pci/devices/{slot}");
        let hex = |f: &str| {
            read(root, &format!("{base}/{f}"))
                .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        };
        let (Some(class), Some(vendor), Some(device)) =
            (hex("class"), hex("vendor"), hex("device"))
        else {
            continue;
        };
        out.push(PciDevice {
            slot,
            class,
            vendor,
            device,
        });
    }
    out
}

pub fn nics(root: &Path) -> Vec<Nic> {
    let mut out = Vec::new();
    for name in list(root, "/sys/class/net") {
        if name == "lo" {
            continue;
        }
        let base = format!("/sys/class/net/{name}");
        let physical = root.join(format!("sys/class/net/{name}/device")).exists();
        out.push(Nic {
            wireless: root.join(format!("sys/class/net/{name}/wireless")).exists()
                || root.join(format!("sys/class/net/{name}/phy80211")).exists(),
            speed: read(root, &format!("{base}/speed"))
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|s| *s > 0)
                .map(|s| s as u32),
            up: read(root, &format!("{base}/operstate")).is_some_and(|s| s == "up"),
            physical,
            name,
        });
    }
    out
}

pub fn usb(root: &Path) -> Vec<UsbDevice> {
    let mut out = Vec::new();
    for name in list(root, "/sys/bus/usb/devices") {
        // (interfaces have a ':' in their name; root hubs are the Linux
        // Foundation's)
        if name.contains(':') {
            continue;
        }
        let base = format!("/sys/bus/usb/devices/{name}");
        if read(root, &format!("{base}/idVendor")).as_deref() == Some("1d6b") {
            continue;
        }
        let Some(product) = read(root, &format!("{base}/product")) else {
            continue;
        };
        out.push(UsbDevice {
            product,
            maker: read(root, &format!("{base}/manufacturer")),
            speed: read(root, &format!("{base}/speed")).and_then(|s| s.parse::<f32>().ok()),
        });
    }
    out
}

/// Each processor's busy and total time (jiffies) from `/proc/stat`.
pub fn cpu_times(root: &Path) -> Vec<(u64, u64)> {
    let text = fs::read_to_string(root.join("proc/stat")).unwrap_or_default();
    text.lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(|b| b.is_ascii_digit()))
        .map(|l| {
            let v: Vec<u64> = l
                .split_whitespace()
                .skip(1)
                .filter_map(|x| x.parse().ok())
                .collect();
            let total: u64 = v.iter().take(8).sum();
            // idle and iowait
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            (total.saturating_sub(idle), total)
        })
        .collect()
}

/// Which disk (by name) holds the file system on device `dev`: a disk, a
/// partition of one, or a mapped device on one.
pub fn disk_of(disks: &[Disk], dev: (u32, u32)) -> Option<usize> {
    let direct = |dev: (u32, u32)| {
        disks
            .iter()
            .position(|d| d.dev == dev || d.partitions.iter().any(|p| p.dev == dev))
    };
    let i = direct(dev)?;
    if disks[i].below.is_empty() {
        return Some(i);
    }
    // a mapped device: the first disk below it that is one
    for b in &disks[i].below {
        if let Some(j) = disks
            .iter()
            .position(|d| &d.name == b || d.partitions.iter().any(|p| &p.name == b))
            && j != i
        {
            return Some(j);
        }
    }
    Some(i)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testdir::Scratch;

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel.trim_start_matches('/'));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn put_raw(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel.trim_start_matches('/'));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// An SMBIOS type 17 structure.
    fn type17(size_mb: u16, kind: u8, speed: u16, strings: &[&str]) -> Vec<u8> {
        let mut r = vec![0u8; 0x28];
        r[0] = 17;
        r[1] = 0x28;
        r[0x0c..0x0e].copy_from_slice(&size_mb.to_le_bytes());
        r[0x10] = 1; // device locator: string 1
        r[0x12] = kind;
        r[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        r[0x17] = 2; // manufacturer
        r[0x1a] = 3; // part number
        for s in strings {
            r.extend_from_slice(s.as_bytes());
            r.push(0);
        }
        r.push(0);
        r
    }

    pub fn fixture(root: &Path) {
        put(
            root,
            "/proc/cpuinfo",
            "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Test CPU 9000\nphysical id\t: 0\ncore id\t\t: 0\ncpu MHz\t\t: 3200.000\n\nprocessor\t: 1\nmodel name\t: Test CPU 9000\nphysical id\t: 0\ncore id\t\t: 1\ncpu MHz\t\t: 3100.5\n\nprocessor\t: 2\nphysical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 3\nphysical id\t: 0\ncore id\t\t: 1\n",
        );
        put(
            root,
            "/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq",
            "4800000\n",
        );
        put(
            root,
            "/proc/meminfo",
            "MemTotal:       32768000 kB\nMemFree:         1000000 kB\nMemAvailable:   16384000 kB\nSwapTotal:       2097152 kB\nSwapFree:        2097152 kB\n",
        );
        put(
            root,
            "/proc/stat",
            "cpu  10 0 10 80 0 0 0 0 0 0\ncpu0 5 0 5 40 0 0 0 0 0 0\ncpu1 1 0 1 98 0 0 0 0 0 0\ncpu2 0 0 0 100 0 0 0 0 0 0\ncpu3 50 0 0 50 0 0 0 0 0 0\nintr 1 2 3\n",
        );
        put(root, "/sys/class/dmi/id/board_vendor", "ACME\n");
        put(root, "/sys/class/dmi/id/board_name", "Z-Board 1\n");
        put(root, "/sys/class/dmi/id/board_version", "Default string\n");
        put(root, "/sys/class/dmi/id/bios_version", "1.2.3\n");
        put_raw(
            root,
            "/sys/firmware/dmi/entries/17-0/raw",
            &type17(16384, 0x22, 5600, &["DIMM_A1", "Maker", "PART-1"]),
        );
        put_raw(
            root,
            "/sys/firmware/dmi/entries/17-1/raw",
            &type17(0, 0x22, 0, &["DIMM_A2", "Unknown", "Unknown"]),
        );
        put_raw(
            root,
            "/sys/firmware/dmi/entries/17-2/raw",
            &type17(16384, 0x22, 5600, &["DIMM_B1", "Maker", "PART-1"]),
        );
        put(root, "/sys/firmware/dmi/entries/16-0/raw", "x");
        // a disk with two partitions, a mapped device on the second, a loop
        put(root, "/sys/block/nvme0n1/size", "1953525168\n");
        put(root, "/sys/block/nvme0n1/dev", "259:0\n");
        put(root, "/sys/block/nvme0n1/device/model", "Fast SSD 1TB\n");
        put(root, "/sys/block/nvme0n1/queue/rotational", "0\n");
        put(root, "/sys/block/nvme0n1/removable", "0\n");
        put(root, "/sys/block/nvme0n1/nvme0n1p1/dev", "259:1\n");
        put(root, "/sys/block/nvme0n1/nvme0n1p1/size", "1048576\n");
        put(root, "/sys/block/nvme0n1/nvme0n1p2/dev", "259:2\n");
        put(root, "/sys/block/nvme0n1/nvme0n1p2/size", "1952476592\n");
        put(root, "/sys/block/dm-0/size", "1952474544\n");
        put(root, "/sys/block/dm-0/dev", "253:0\n");
        put(root, "/sys/block/dm-0/dm/name", "cryptroot\n");
        put(root, "/sys/block/dm-0/slaves/nvme0n1p2/x", "");
        put(root, "/sys/block/sda/size", "7814037168\n");
        put(root, "/sys/block/sda/dev", "8:0\n");
        put(root, "/sys/block/sda/queue/rotational", "1\n");
        put(root, "/sys/block/sda/device/model", "Big Disk\n");
        put(root, "/sys/block/loop0/size", "100\n");
        put(
            root,
            "/sys/bus/pci/devices/0000:01:00.0/class",
            "0x030000\n",
        );
        put(root, "/sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
        put(root, "/sys/bus/pci/devices/0000:01:00.0/device", "0x2684\n");
        put(
            root,
            "/sys/bus/pci/devices/0000:02:00.0/class",
            "0x020000\n",
        );
        put(root, "/sys/bus/pci/devices/0000:02:00.0/vendor", "0x8086\n");
        put(root, "/sys/bus/pci/devices/0000:02:00.0/device", "0x15f3\n");
        put(root, "/sys/class/net/lo/operstate", "unknown\n");
        put(root, "/sys/class/net/eth0/operstate", "up\n");
        put(root, "/sys/class/net/eth0/speed", "1000\n");
        put(root, "/sys/class/net/eth0/device/x", "");
        put(root, "/sys/class/net/wlan0/operstate", "down\n");
        put(root, "/sys/class/net/wlan0/speed", "-1\n");
        put(root, "/sys/class/net/wlan0/wireless/x", "");
        put(root, "/sys/class/net/wlan0/device/x", "");
        put(root, "/sys/class/net/br0/operstate", "up\n");
        put(root, "/sys/bus/usb/devices/usb1/idVendor", "1d6b\n");
        put(
            root,
            "/sys/bus/usb/devices/usb1/product",
            "xHCI Host Controller\n",
        );
        put(root, "/sys/bus/usb/devices/1-2/idVendor", "046d\n");
        put(root, "/sys/bus/usb/devices/1-2/product", "Keyboard\n");
        put(root, "/sys/bus/usb/devices/1-2/manufacturer", "Keys Inc\n");
        put(root, "/sys/bus/usb/devices/1-2/speed", "12\n");
        put(root, "/sys/bus/usb/devices/1-2:1.0/product", "interface\n");
        put(
            root,
            "/etc/os-release",
            "NAME=Test\nPRETTY_NAME=\"Test Linux 1\"\n",
        );
    }

    #[test]
    fn reads_a_fixture_machine() {
        let dir = Scratch::new("hw");
        fixture(dir.path());
        let m = detect(dir.path());
        assert_eq!(m.os, "Test Linux 1");
        assert_eq!(m.cpu.model, "Test CPU 9000");
        assert_eq!(m.cpu.cores.len(), 4);
        assert_eq!(m.cpu.physical, 2);
        assert_eq!(m.cpu.packages, 1);
        assert_eq!(m.cpu.cores[0].mhz, 3200);
        assert_eq!(m.cpu.cores[0].max_mhz, 4800);
        assert_eq!(m.memory.total, 32_768_000 * 1024);
        assert_eq!(m.memory.available, 16_384_000 * 1024);
        assert_eq!(m.memory.modules.len(), 3);
        assert_eq!(m.memory.modules[0].slot, "DIMM_A1");
        assert_eq!(m.memory.modules[0].size, 16 << 30);
        assert_eq!(m.memory.modules[0].speed, Some(5600));
        assert_eq!(m.memory.modules[0].maker.as_deref(), Some("Maker"));
        assert_eq!(m.memory.modules[1].size, 0, "an empty slot");
        assert_eq!(m.memory.modules[1].maker, None);
        assert_eq!(m.memory.kind.as_deref(), Some("DDR5"));
        assert_eq!(m.board.vendor.as_deref(), Some("ACME"));
        assert_eq!(m.board.name.as_deref(), Some("Z-Board 1"));
        assert_eq!(m.board.version, None, "a placeholder is no version");
        let names: Vec<&str> = m.disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["dm-0", "nvme0n1", "sda"]);
        let nvme = &m.disks[1];
        assert_eq!(nvme.transport, "NVMe");
        assert_eq!(nvme.partitions.len(), 2);
        assert_eq!(nvme.size, 1_953_525_168 * 512);
        assert!(m.disks[2].rotational);
        // a mount on the mapped device lives on the NVMe disk
        assert_eq!(disk_of(&m.disks, (253, 0)), Some(1));
        assert_eq!(disk_of(&m.disks, (259, 1)), Some(1));
        assert_eq!(disk_of(&m.disks, (8, 0)), Some(2));
        assert_eq!(disk_of(&m.disks, (1, 1)), None);
        assert_eq!(m.pci.len(), 2);
        assert_eq!(m.pci[0].kind(), PciKind::Display);
        assert_eq!(m.pci[0].vendor_name(), "NVIDIA");
        let nics: Vec<&str> = m.nics.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(nics, ["br0", "eth0", "wlan0"]);
        assert!(!m.nics[0].physical);
        assert_eq!(m.nics[1].speed, Some(1000));
        assert!(m.nics[2].wireless && m.nics[2].speed.is_none());
        assert_eq!(m.usb.len(), 1);
        assert_eq!(m.usb[0].product, "Keyboard");
        let t = cpu_times(dir.path());
        assert_eq!(t.len(), 4);
        assert_eq!(t[3], (50, 100));
    }

    #[test]
    fn an_empty_root_gives_an_empty_machine_quietly() {
        let dir = Scratch::new("hw-empty");
        let m = detect(dir.path());
        assert!(m.cpu.cores.is_empty());
        assert_eq!(m.memory.total, 0);
        assert!(m.disks.is_empty() && m.pci.is_empty() && m.nics.is_empty() && m.usb.is_empty());
        assert!(cpu_times(dir.path()).is_empty());
    }

    #[test]
    fn smbios_sizes_and_junk() {
        assert_eq!(
            smbios_memory_device(&type17(0x7fff, 0x1a, 3200, &["A"]))
                .unwrap()
                .size,
            0
        );
        let mut ext = type17(0x7fff, 0x1a, 3200, &["A"]);
        ext[0x1c..0x20].copy_from_slice(&65536u32.to_le_bytes());
        assert_eq!(smbios_memory_device(&ext).unwrap().size, 64 << 30);
        assert_eq!(
            smbios_memory_device(&type17(0x8000 | 512, 0x1a, 0, &[]))
                .unwrap()
                .size,
            512 << 10
        );
        assert!(smbios_memory_device(&[17, 200, 0]).is_none());
        assert!(smbios_memory_device(&[]).is_none());
        assert!(smbios_memory_device(&[16; 40]).is_none());
        // a string index past the strings
        let mut bad = type17(1024, 0x1a, 0, &[]);
        bad[0x10] = 9;
        assert_eq!(smbios_memory_device(&bad).unwrap().slot, "slot");
    }

    #[test]
    fn the_real_machine_reads_without_failing() {
        let m = detect(Path::new("/"));
        // whatever there is, nothing panics; a Linux machine has processors
        if cfg!(target_os = "linux") {
            assert!(!m.cpu.cores.is_empty());
        }
    }
}
