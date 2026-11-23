//! The machine the city runs on, as an island of its own: the mainboard is
//! the ground plan, the processor's cores stand as towers in the socket
//! (their light the load), the memory modules lie in their slots (their
//! light how much is in use), the cards sit in the PCIe slots, the disks
//! at the edge - and every mountpoint's island is tied to its disk by a
//! beam of light.
//!
//! What can be read without root is read ([`linux`]); what cannot is left
//! out quietly. macOS and Windows show the processor count for now.

#[cfg(any(target_os = "linux", test))]
pub mod linux;

use crate::layout::{Building, CityLayout, District, Placed};
use crate::tree::{Entry, FileType, Kind, Listing, NodeId, Tree};
use crate::treemap::Rect;
use std::collections::HashMap;
use std::ffi::OsStr;

#[derive(Clone, Debug, Default)]
pub struct Machine {
    pub host: String,
    pub os: String,
    pub kernel: String,
    pub board: Board,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disks: Vec<Disk>,
    pub pci: Vec<PciDevice>,
    pub nics: Vec<Nic>,
    pub usb: Vec<UsbDevice>,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub vendor: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub system_vendor: Option<String>,
    pub product: Option<String>,
    pub bios_vendor: Option<String>,
    pub bios_version: Option<String>,
    pub bios_date: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Cpu {
    pub model: String,
    pub vendor: String,
    pub packages: u32,
    /// Physical cores (the logical ones are [`Cpu::cores`]).
    pub physical: u32,
    pub cores: Vec<Core>,
}

#[derive(Clone, Debug, Default)]
pub struct Core {
    pub id: u32,
    pub package: u32,
    pub core: u32,
    pub mhz: u32,
    pub max_mhz: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    /// DDR4, DDR5, ... (from the modules)
    pub kind: Option<String>,
    /// Every slot (an empty one has size 0), when readable.
    pub modules: Vec<MemModule>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemModule {
    pub slot: String,
    pub size: u64,
    pub kind: Option<String>,
    /// MT/s
    pub speed: Option<u32>,
    pub maker: Option<String>,
    pub part: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Disk {
    pub name: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub size: u64,
    pub rotational: bool,
    pub removable: bool,
    pub transport: String,
    pub dev: (u32, u32),
    pub partitions: Vec<Partition>,
    /// A mapped device's disks below it (their names).
    pub below: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Partition {
    pub name: String,
    pub dev: (u32, u32),
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PciKind {
    Display,
    Network,
    Storage,
    Usb,
    Audio,
    Bridge,
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct PciDevice {
    pub slot: String,
    pub class: u32,
    pub vendor: u32,
    pub device: u32,
}

impl PciDevice {
    pub fn kind(&self) -> PciKind {
        match self.class >> 16 {
            0x03 => PciKind::Display,
            0x02 => PciKind::Network,
            0x01 => PciKind::Storage,
            0x04 => PciKind::Audio,
            0x06 => PciKind::Bridge,
            0x0c if (self.class >> 8) & 0xff == 0x03 => PciKind::Usb,
            _ => PciKind::Other,
        }
    }
    pub fn vendor_name(&self) -> String {
        match self.vendor {
            0x10de => "NVIDIA".into(),
            0x1002 | 0x1022 => "AMD".into(),
            0x8086 | 0x8087 => "Intel".into(),
            0x1af4 => "virtio".into(),
            0x14e4 => "Broadcom".into(),
            0x10ec => "Realtek".into(),
            0x15b3 => "Mellanox".into(),
            0x144d => "Samsung".into(),
            0x1c5c => "SK hynix".into(),
            0x15b7 => "Sandisk / WD".into(),
            0x1e0f => "Kioxia".into(),
            0x106b => "Apple".into(),
            0x1414 => "Microsoft".into(),
            0x1d0f => "Amazon".into(),
            0x1b36 => "QEMU".into(),
            0x168c => "Qualcomm Atheros".into(),
            0x1b21 => "ASMedia".into(),
            v => format!("vendor {v:04x}"),
        }
    }
    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            PciKind::Display => "graphics",
            PciKind::Network => "network",
            PciKind::Storage => "storage controller",
            PciKind::Usb => "USB controller",
            PciKind::Audio => "audio",
            PciKind::Bridge => "bridge",
            PciKind::Other => "device",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Nic {
    pub name: String,
    pub wireless: bool,
    /// Mbit/s
    pub speed: Option<u32>,
    pub up: bool,
    /// A device (not a bridge, a tunnel, a container's link).
    pub physical: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UsbDevice {
    pub product: String,
    pub maker: Option<String>,
    /// Mbit/s
    pub speed: Option<f32>,
}

/// This machine (an empty one where the system says nothing readable).
pub fn detect() -> Machine {
    #[cfg(target_os = "linux")]
    {
        linux::detect(std::path::Path::new("/"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        Machine {
            os: std::env::consts::OS.into(),
            cpu: Cpu {
                model: "processor".into(),
                packages: 1,
                physical: n as u32,
                cores: (0..n as u32)
                    .map(|id| Core {
                        id,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// A made-up machine (the screenshots and tests: no real machine's
/// details in a picture).
pub fn demo_machine() -> Machine {
    let core = |id: u32| Core {
        id,
        package: 0,
        core: id / 2,
        mhz: 3600 + (id * 37) % 900,
        max_mhz: 5200,
    };
    let module = |slot: &str, gib: u64| MemModule {
        slot: slot.into(),
        size: gib << 30,
        kind: Some("DDR5".into()),
        speed: (gib > 0).then_some(5600),
        maker: (gib > 0).then(|| "Example Memory".into()),
        part: None,
    };
    let disk = |name: &str, model: &str, size: u64, rot: bool, transport: &str, maj: u32| Disk {
        name: name.into(),
        model: Some(model.into()),
        vendor: None,
        size,
        rotational: rot,
        removable: false,
        transport: transport.into(),
        dev: (maj, 0),
        partitions: vec![Partition {
            name: format!("{name}p1"),
            dev: (maj, 1),
            size,
        }],
        below: vec![],
    };
    Machine {
        host: "demo".into(),
        os: "Demo Linux".into(),
        kernel: "6.0.0".into(),
        board: Board {
            vendor: Some("Example".into()),
            name: Some("Matrix Board X1".into()),
            ..Default::default()
        },
        cpu: Cpu {
            model: "Example CPU 16-core".into(),
            vendor: "Example".into(),
            packages: 1,
            physical: 8,
            cores: (0..16).map(core).collect(),
        },
        memory: Memory {
            total: 64 << 30,
            available: 40 << 30,
            swap_total: 8 << 30,
            swap_free: 8 << 30,
            kind: Some("DDR5".into()),
            modules: vec![
                module("DIMM_A1", 32),
                module("DIMM_A2", 0),
                module("DIMM_B1", 32),
                module("DIMM_B2", 0),
            ],
        },
        disks: vec![
            disk(
                "nvme0n1",
                "Fast NVMe 2TB",
                2_000_398_934_016,
                false,
                "NVMe",
                259,
            ),
            disk(
                "sda",
                "Archive Disk 8TB",
                8_001_563_222_016,
                true,
                "SATA / SCSI",
                8,
            ),
        ],
        pci: vec![
            PciDevice {
                slot: "0000:01:00.0".into(),
                class: 0x030000,
                vendor: 0x10de,
                device: 0x2684,
            },
            PciDevice {
                slot: "0000:02:00.0".into(),
                class: 0x020000,
                vendor: 0x8086,
                device: 0x15f3,
            },
            PciDevice {
                slot: "0000:03:00.0".into(),
                class: 0x010802,
                vendor: 0x144d,
                device: 0xa80a,
            },
            PciDevice {
                slot: "0000:00:14.0".into(),
                class: 0x0c0330,
                vendor: 0x8086,
                device: 0x7a60,
            },
            PciDevice {
                slot: "0000:00:1f.3".into(),
                class: 0x040300,
                vendor: 0x8086,
                device: 0x7a50,
            },
        ],
        nics: vec![
            Nic {
                name: "eth0".into(),
                wireless: false,
                speed: Some(2500),
                up: true,
                physical: true,
            },
            Nic {
                name: "wlan0".into(),
                wireless: true,
                speed: None,
                up: false,
                physical: true,
            },
        ],
        usb: vec![
            UsbDevice {
                product: "Keyboard".into(),
                maker: None,
                speed: Some(12.0),
            },
            UsbDevice {
                product: "Mouse".into(),
                maker: None,
                speed: Some(12.0),
            },
            UsbDevice {
                product: "Card Reader".into(),
                maker: None,
                speed: Some(480.0),
            },
        ],
    }
}

/// The disk (index) a file system on device `dev` lives on.
pub fn disk_of(m: &Machine, dev: (u32, u32)) -> Option<usize> {
    if dev == (0, 0) {
        return None;
    }
    #[cfg(any(target_os = "linux", test))]
    {
        linux::disk_of(&m.disks, dev)
    }
    #[cfg(not(any(target_os = "linux", test)))]
    {
        let _ = m;
        None
    }
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// What a part is (its building's role: the live light, the beams).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Board,
    Core(u32),
    Module,
    MemoryInUse,
    Disk(usize),
    Card,
    Port,
}

/// The machine's island: a tree of its parts (so that picking, the info
/// panel, labels and the teleport work as on any island), its layout, the
/// details of each part, and which building is which part.
#[derive(Clone, Debug, Default)]
pub struct HwIsland {
    pub tree: Tree,
    pub layout: CityLayout,
    pub info: HashMap<NodeId, Vec<(String, String)>>,
    pub parts: HashMap<NodeId, Part>,
}

/// The island's side.
pub const SIDE: f32 = 220.0;

struct Builder {
    tree: Tree,
    layout: CityLayout,
    info: HashMap<NodeId, Vec<(String, String)>>,
    parts: HashMap<NodeId, Part>,
    /// The district being filled (index), and its node.
    current: Option<(usize, NodeId)>,
}

impl Builder {
    fn node(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: Kind,
        info: Vec<(String, String)>,
    ) -> NodeId {
        let id = self
            .tree
            .push(Entry {
                parent,
                name: OsStr::new(name).into(),
                kind,
                size: 0,
                mtime: 0,
            })
            .unwrap_or(0);
        if kind == Kind::Dir {
            self.tree.nodes[id as usize].listing = Listing::Listed;
        }
        self.layout.placed.push(Placed::Nowhere);
        self.info.insert(id, info);
        id
    }

    /// A zone of the board (a district) at `rect`.
    fn zone(&mut self, name: &str, rect: Rect, info: Vec<(String, String)>) -> NodeId {
        self.close();
        let id = self.node(0, name, Kind::Dir, info);
        let index = self.layout.districts.len();
        let first = self.layout.buildings.len() as u32;
        self.layout.placed[id as usize] = Placed::District(index as u32);
        self.layout.districts.push(District {
            node: id,
            rect,
            floor: 0.25,
            depth: 1,
            end: index as u32 + 1,
            first,
            own_end: first,
            sub_end: first,
            top: 0.25,
            block_height: 2.0,
            ftype: FileType::Binary,
            glow: 0.3,
            listing: Listing::Listed,
        });
        self.current = Some((index, id));
        id
    }

    /// A part (a building) in the current zone.
    #[allow(clippy::too_many_arguments)]
    fn part(
        &mut self,
        name: &str,
        rect: Rect,
        height: f32,
        ftype: FileType,
        glow: f32,
        part: Part,
        info: Vec<(String, String)>,
    ) -> NodeId {
        let parent = self.current.map_or(0, |c| c.1);
        let id = self.node(parent, name, Kind::File, info);
        self.layout.placed[id as usize] = Placed::Building(self.layout.buildings.len() as u32);
        let base = self
            .current
            .map_or(0.0, |(d, _)| self.layout.districts[d].floor);
        self.layout.buildings.push(Building {
            node: id,
            rect,
            base,
            height,
            ftype,
            glow,
            hash: crate::math::hash32(name.as_bytes()),
        });
        self.parts.insert(id, part);
        if let Some((d, _)) = self.current {
            let dist = &mut self.layout.districts[d];
            dist.own_end = self.layout.buildings.len() as u32;
            dist.sub_end = dist.own_end;
            dist.top = dist.top.max(base + height);
        }
        id
    }

    fn close(&mut self) {
        self.current = None;
    }
}

fn kv(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// Lays the machine out as its island.
pub fn island(m: &Machine) -> HwIsland {
    let title = "This machine";
    let mut b = Builder {
        tree: Tree::new(OsStr::new(title), 0),
        layout: CityLayout {
            side: SIDE,
            nodes: 0,
            placed: vec![Placed::Nowhere],
            ..Default::default()
        },
        info: HashMap::new(),
        parts: HashMap::new(),
        current: None,
    };
    b.tree.nodes[0].listing = Listing::Listed;
    let mut board_info = Vec::new();
    let board_name = [m.board.vendor.as_deref(), m.board.name.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if !board_name.is_empty() {
        board_info.push(kv("mainboard", board_name.clone()));
    }
    if let Some(v) = &m.board.version {
        board_info.push(kv("board version", v.clone()));
    }
    let system = [m.board.system_vendor.as_deref(), m.board.product.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if !system.is_empty() {
        board_info.push(kv("system", system));
    }
    if let Some(v) = &m.board.bios_version {
        let date = m.board.bios_date.clone().unwrap_or_default();
        board_info.push(kv("firmware", format!("{v} {date}").trim().to_string()));
    }
    if !m.os.is_empty() {
        board_info.push(kv("system software", m.os.clone()));
    }
    if !m.kernel.is_empty() {
        board_info.push(kv("kernel", m.kernel.clone()));
    }
    b.info.insert(0, board_info.clone());
    b.layout.districts.push(District {
        node: 0,
        rect: Rect::new(0.0, 0.0, SIDE, SIDE),
        floor: 0.0,
        depth: 0,
        end: 1,
        first: 0,
        own_end: 0,
        sub_end: 0,
        top: 0.2,
        block_height: 1.0,
        ftype: FileType::Binary,
        glow: 0.2,
        listing: Listing::Listed,
    });
    b.layout.placed[0] = Placed::District(0);

    // the processor: a socket with a tower per logical core
    let cores = m.cpu.cores.len().max(1);
    let socket = Rect::new(36.0, 40.0, 104.0, 108.0);
    let cpu_info = vec![
        kv("model", m.cpu.model.clone()),
        kv("vendor", m.cpu.vendor.clone()),
        kv("packages", m.cpu.packages.to_string()),
        kv(
            "cores",
            format!("{} physical, {} logical", m.cpu.physical, m.cpu.cores.len()),
        ),
    ];
    b.zone("CPU", socket, cpu_info);
    let grid = (cores as f32).sqrt().ceil() as usize;
    let inner = socket.inset(6.0);
    let cell = inner.w().min(inner.h()) / grid as f32;
    for (i, c) in m.cpu.cores.iter().enumerate() {
        let (gx, gz) = (i % grid, i / grid);
        let x0 = inner.x0 + gx as f32 * cell;
        let z0 = inner.z0 + gz as f32 * cell;
        let r = Rect::new(x0, z0, x0 + cell, z0 + cell).shrunk(0.16);
        let ghz = c.max_mhz.max(c.mhz) as f32 / 1000.0;
        let height = 10.0 + 4.0 * ghz.min(6.0);
        let mut info = vec![
            kv("logical core", c.id.to_string()),
            kv(
                "physical core",
                format!("{} of package {}", c.core, c.package),
            ),
        ];
        if c.mhz > 0 {
            info.push(kv("clock", format!("{} MHz", c.mhz)));
        }
        if c.max_mhz > 0 {
            info.push(kv("max clock", format!("{} MHz", c.max_mhz)));
        }
        b.part(
            &format!("core {}", c.id),
            r,
            height,
            FileType::Code,
            0.1,
            Part::Core(c.id),
            info,
        );
    }

    // memory: a slot per module (empty ones low), the use beside them
    let ram = Rect::new(118.0, 30.0, 170.0, 118.0);
    let used = m.memory.total.saturating_sub(m.memory.available);
    let mut mem_info = vec![
        kv("installed", human_bytes(m.memory.total)),
        kv("in use", human_bytes(used)),
    ];
    if let Some(k) = &m.memory.kind {
        mem_info.push(kv("type", k.clone()));
    }
    if !m.memory.modules.is_empty() {
        let filled = m.memory.modules.iter().filter(|x| x.size > 0).count();
        mem_info.push(kv(
            "slots",
            format!("{} ({} filled)", m.memory.modules.len(), filled),
        ));
    } else {
        mem_info.push(kv("modules", "not readable without root"));
    }
    if m.memory.swap_total > 0 {
        mem_info.push(kv(
            "swap",
            format!(
                "{} of {} in use",
                human_bytes(m.memory.swap_total - m.memory.swap_free),
                human_bytes(m.memory.swap_total)
            ),
        ));
    }
    b.zone("Memory", ram, mem_info);
    let modules: Vec<MemModule> = if m.memory.modules.is_empty() {
        // (unknown modules: the memory as one block)
        vec![MemModule {
            slot: "memory".into(),
            size: m.memory.total,
            ..Default::default()
        }]
    } else {
        m.memory.modules.clone()
    };
    let slots = modules.len().max(1);
    let pitch = ((ram.w() - 14.0) / slots as f32).min(7.0);
    for (i, module) in modules.iter().enumerate() {
        let x0 = ram.x0 + 4.0 + i as f32 * pitch;
        let r = Rect::new(x0, ram.z0 + 6.0, x0 + pitch * 0.45, ram.z1 - 6.0);
        let gib = module.size as f32 / (1u64 << 30) as f32;
        let height = if module.size == 0 {
            0.5
        } else {
            3.0 + 2.2 * (gib.max(0.5)).log2().max(0.0)
        };
        let mut info = vec![kv("slot", module.slot.clone())];
        if module.size == 0 {
            info.push(kv("module", "empty"));
        } else {
            info.push(kv("size", human_bytes(module.size)));
        }
        if let Some(k) = &module.kind {
            info.push(kv("type", k.clone()));
        }
        if let Some(s) = module.speed {
            info.push(kv("speed", format!("{s} MT/s")));
        }
        if let Some(mk) = &module.maker {
            info.push(kv("maker", mk.clone()));
        }
        if let Some(p) = &module.part {
            info.push(kv("part", p.clone()));
        }
        let ftype = if module.size == 0 {
            FileType::Other
        } else {
            FileType::Data
        };
        b.part(&module.slot, r, height, ftype, 0.2, Part::Module, info);
    }
    let share = if m.memory.total > 0 {
        used as f32 / m.memory.total as f32
    } else {
        0.0
    };
    let gauge = Rect::new(
        ram.x1 - 8.0,
        ram.z0 + 6.0,
        ram.x1 - 4.0,
        ram.z0 + 6.0 + (ram.h() - 12.0) * share.clamp(0.02, 1.0),
    );
    b.part(
        "memory in use",
        gauge,
        2.0,
        FileType::Document,
        share,
        Part::MemoryInUse,
        vec![kv(
            "in use",
            format!(
                "{} of {} ({:.0} %)",
                human_bytes(used),
                human_bytes(m.memory.total),
                share * 100.0
            ),
        )],
    );

    // the cards in the PCIe slots
    let cards: Vec<&PciDevice> = m
        .pci
        .iter()
        .filter(|d| {
            matches!(
                d.kind(),
                PciKind::Display
                    | PciKind::Network
                    | PciKind::Storage
                    | PciKind::Usb
                    | PciKind::Audio
            )
        })
        .collect();
    let slots_zone = Rect::new(24.0, 126.0, 120.0, 196.0);
    b.zone(
        "PCI devices",
        slots_zone,
        vec![
            kv("devices", m.pci.len().to_string()),
            kv("shown", cards.len().min(10).to_string()),
        ],
    );
    for (i, d) in cards.iter().take(10).enumerate() {
        let z0 = slots_zone.z0 + 4.0 + i as f32 * 6.4;
        let (len, height, ftype) = match d.kind() {
            PciKind::Display => (80.0, 9.0, FileType::Video),
            PciKind::Network => (40.0, 4.0, FileType::Config),
            PciKind::Storage => (32.0, 3.5, FileType::Archive),
            _ => (26.0, 3.0, FileType::Binary),
        };
        let r = Rect::new(slots_zone.x0 + 4.0, z0, slots_zone.x0 + 4.0 + len, z0 + 3.2);
        let name = format!("{} {}", d.vendor_name(), d.kind_name());
        b.part(
            &name,
            r,
            height,
            ftype,
            0.2,
            Part::Card,
            vec![
                kv("slot", d.slot.clone()),
                kv("kind", d.kind_name()),
                kv("vendor", d.vendor_name()),
                kv("ids", format!("{:04x}:{:04x}", d.vendor, d.device)),
            ],
        );
    }

    // the disks at the edge, big ones bigger
    let disk_zone = Rect::new(132.0, 130.0, 206.0, 206.0);
    let disks: Vec<(usize, &Disk)> = m
        .disks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.below.is_empty())
        .collect();
    b.zone(
        "Disks",
        disk_zone,
        vec![kv("disks", disks.len().to_string())],
    );
    let per_row = 3usize;
    for (k, (i, d)) in disks.iter().take(12).enumerate() {
        let (gx, gz) = (k % per_row, k / per_row);
        let w = (disk_zone.w() - 8.0) / per_row as f32;
        let x0 = disk_zone.x0 + 4.0 + gx as f32 * w;
        let z0 = disk_zone.z0 + 4.0 + gz as f32 * 18.0;
        let r = Rect::new(x0, z0, x0 + w, z0 + 15.0).shrunk(0.1);
        let gb = d.size as f32 / 1e9;
        let height = if d.rotational { 6.0 } else { 2.5 } + 2.0 * gb.max(1.0).log10();
        let mut info = vec![
            kv("device", d.name.clone()),
            kv("size", human_bytes(d.size)),
            kv("kind", d.transport.clone()),
        ];
        if let Some(mo) = &d.model {
            info.push(kv("model", mo.clone()));
        }
        info.push(kv(
            "medium",
            if d.rotational {
                "spinning disk"
            } else {
                "solid state"
            },
        ));
        if !d.partitions.is_empty() {
            info.push(kv(
                "partitions",
                d.partitions
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let name = d.model.clone().unwrap_or_else(|| d.name.clone());
        b.part(
            &name,
            r,
            height,
            FileType::Archive,
            0.4,
            Part::Disk(*i),
            info,
        );
    }

    // the back panel: network and USB
    let panel = Rect::new(20.0, 6.0, 206.0, 24.0);
    let nics: Vec<&Nic> = m.nics.iter().filter(|n| n.physical).collect();
    b.zone(
        "Ports",
        panel,
        vec![
            kv("network", nics.len().to_string()),
            kv("USB devices", m.usb.len().to_string()),
        ],
    );
    let mut x = panel.x0 + 3.0;
    for n in nics.iter().take(6) {
        let r = Rect::new(x, panel.z0 + 3.0, x + 9.0, panel.z1 - 3.0);
        let mut info = vec![
            kv("interface", n.name.clone()),
            kv("kind", if n.wireless { "wireless" } else { "wired" }),
            kv("state", if n.up { "up" } else { "down" }),
        ];
        if let Some(s) = n.speed {
            info.push(kv("speed", format!("{s} Mbit/s")));
        }
        b.part(
            &n.name,
            r,
            if n.wireless { 7.0 } else { 3.5 },
            FileType::Config,
            if n.up { 0.7 } else { 0.0 },
            Part::Port,
            info,
        );
        x += 12.0;
    }
    for u in m.usb.iter().take(10) {
        let r = Rect::new(x, panel.z0 + 5.0, x + 5.0, panel.z1 - 5.0);
        let mut info = vec![kv("device", u.product.clone())];
        if let Some(mk) = &u.maker {
            info.push(kv("maker", mk.clone()));
        }
        if let Some(s) = u.speed {
            info.push(kv("speed", format!("{s} Mbit/s")));
        }
        b.part(&u.product, r, 2.0, FileType::Audio, 0.3, Part::Port, info);
        x += 7.5;
    }
    b.close();
    // the board's own label block (the chipset's place)
    b.zone("Mainboard", Rect::new(20.0, 26.0, 112.0, 38.0), board_info);
    b.part(
        if board_name.is_empty() {
            "mainboard"
        } else {
            &board_name
        },
        Rect::new(22.0, 28.0, 34.0, 36.0),
        1.5,
        FileType::Binary,
        0.3,
        Part::Board,
        vec![],
    );
    b.close();

    // the root district spans everything; tops
    let count = b.layout.districts.len() as u32;
    let total = b.layout.buildings.len() as u32;
    let top = b.layout.districts.iter().map(|d| d.top).fold(1.0, f32::max);
    let root = &mut b.layout.districts[0];
    root.end = count;
    root.sub_end = total;
    root.top = top;
    b.layout.top = top;
    b.layout.nodes = b.tree.len();
    HwIsland {
        tree: b.tree,
        layout: b.layout,
        info: b.info,
        parts: b.parts,
    }
}

/// Samples the processor's load and the memory in use, once a second, on a
/// thread of its own; each core's busy share 0..1 and the memory's.
pub struct Sampler {
    rx: std::sync::mpsc::Receiver<(Vec<f32>, f32)>,
    pub last: Option<(Vec<f32>, f32)>,
}

impl Sampler {
    pub fn start(wake: crate::walk::Wake) -> Sampler {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("afm-hw-sample".into())
            .spawn(move || {
                #[cfg(target_os = "linux")]
                {
                    let root = std::path::Path::new("/");
                    let mut prev = linux::cpu_times(root);
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        let now = linux::cpu_times(root);
                        let loads: Vec<f32> = now
                            .iter()
                            .zip(prev.iter().chain(std::iter::repeat(&(0, 0))))
                            .map(|(n, p)| {
                                let busy = n.0.saturating_sub(p.0) as f32;
                                let total = n.1.saturating_sub(p.1) as f32;
                                if total > 0.0 {
                                    (busy / total).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                }
                            })
                            .collect();
                        prev = now;
                        let mem = linux::memory(root);
                        let share = if mem.total > 0 {
                            mem.total.saturating_sub(mem.available) as f32 / mem.total as f32
                        } else {
                            0.0
                        };
                        if tx.send((loads, share)).is_err() {
                            break;
                        }
                        wake();
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = (tx, wake);
                }
            });
        Sampler { rx, last: None }
    }

    /// The newest sample, if a new one came.
    pub fn poll(&mut self) -> Option<&(Vec<f32>, f32)> {
        let mut got = false;
        while let Ok(s) = self.rx.try_recv() {
            self.last = Some(s);
            got = true;
        }
        if got { self.last.as_ref() } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Scratch;

    #[test]
    fn the_fixture_machine_becomes_an_island() {
        let dir = Scratch::new("hw-island");
        linux::tests::fixture(dir.path());
        let m = linux::detect(dir.path());
        let h = island(&m);
        // the tree and the layout agree
        assert_eq!(h.layout.placed.len(), h.tree.len());
        assert_eq!(
            h.layout.buildings.len() + h.layout.districts.len(),
            h.tree.len()
        );
        let cores = h
            .parts
            .values()
            .filter(|p| matches!(p, Part::Core(_)))
            .count();
        assert_eq!(cores, 4);
        let disks: Vec<usize> = h
            .parts
            .values()
            .filter_map(|p| {
                if let Part::Disk(i) = p {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(disks.len(), 2, "the mapped device is no disk of its own");
        assert_eq!(h.parts.values().filter(|p| **p == Part::Module).count(), 3);
        // every building lies on the board, inside its zone
        for bld in &h.layout.buildings {
            let r = bld.rect;
            assert!(
                r.x0 >= 0.0 && r.x1 <= SIDE && r.z0 >= 0.0 && r.z1 <= SIDE,
                "{r:?}"
            );
            assert!(bld.height > 0.0);
        }
        for (i, d) in h.layout.districts.iter().enumerate().skip(1) {
            for bld in &h.layout.buildings[d.first as usize..d.own_end as usize] {
                assert!(
                    bld.rect.x0 >= d.rect.x0 - 1e-3 && bld.rect.x1 <= d.rect.x1 + 1e-3,
                    "zone {i}"
                );
            }
        }
        assert!(
            h.info[&0]
                .iter()
                .any(|(k, v)| k == "mainboard" && v == "ACME Z-Board 1")
        );
        // a mountpoint on the encrypted root is tied to the NVMe disk
        assert_eq!(disk_of(&m, (253, 0)), Some(1));
    }

    #[test]
    fn an_empty_machine_still_lays_out() {
        let h = island(&Machine::default());
        assert!(h.layout.buildings.len() >= 2);
        assert_eq!(h.layout.placed.len(), h.tree.len());
    }

    #[test]
    fn bytes_for_people() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(32 << 30), "32.0 GiB");
        assert_eq!(human_bytes(1 << 40), "1.0 TiB");
    }
}
