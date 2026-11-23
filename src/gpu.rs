//! Which GPU draws the city: the dedicated one by default (high
//! performance), the integrated one when the setting says low power, never
//! a software renderer while a real GPU is there. `WGPU_ADAPTER_NAME` (a
//! part of the adapter's name) picks one by name, `WGPU_BACKEND` (`vulkan`,
//! `metal`, `dx12`, `gl`) the API, `WGPU_POWER_PREF` (`high`, `low`) the
//! preference when the setting is *auto*. Vendor-neutral: whatever wgpu
//! reaches.

use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

/// The setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preference {
    /// High performance unless `WGPU_POWER_PREF` says otherwise.
    Auto,
    HighPerformance,
    LowPower,
}

impl Preference {
    pub fn from_key(key: &str) -> Preference {
        match key {
            "high" => Preference::HighPerformance,
            "low" => Preference::LowPower,
            _ => Preference::Auto,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Preference::Auto => "auto",
            Preference::HighPerformance => "high",
            Preference::LowPower => "low",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Preference::Auto => "Automatic (the fastest GPU)",
            Preference::HighPerformance => "High performance (a dedicated GPU)",
            Preference::LowPower => "Low power (an integrated GPU)",
        }
    }
    /// What it comes to (the environment taken into account).
    fn resolved(self, env: Option<&str>) -> Preference {
        match self {
            Preference::Auto => match env.map(|v| v.trim().to_ascii_lowercase()) {
                Some(v) if v == "low" => Preference::LowPower,
                _ => Preference::HighPerformance,
            },
            p => p,
        }
    }
}

/// Whether an adapter is a software renderer (llvmpipe, lavapipe,
/// SwiftShader, WARP).
pub fn is_software(name: &str, device_type: wgpu::DeviceType) -> bool {
    let n = name.to_ascii_lowercase();
    device_type == wgpu::DeviceType::Cpu
        || [
            "llvmpipe",
            "lavapipe",
            "softpipe",
            "swiftshader",
            "microsoft basic render",
        ]
        .iter()
        .any(|s| n.contains(s))
}

/// How well an adapter fits the preference (higher is better).
pub fn rank(name: &str, device_type: wgpu::DeviceType, pref: Preference) -> u32 {
    use wgpu::DeviceType as D;
    if is_software(name, device_type) {
        return 0;
    }
    match (pref, device_type) {
        (Preference::LowPower, D::IntegratedGpu) => 4,
        (Preference::LowPower, D::DiscreteGpu) => 3,
        (_, D::DiscreteGpu) => 4,
        (_, D::IntegratedGpu) => 3,
        (_, D::VirtualGpu) => 2,
        _ => 1,
    }
}

/// The adapter to use among `adapters`: by `WGPU_ADAPTER_NAME` when set
/// (and one matches), else the best ranked that can draw to `surface`.
pub fn choose(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
    pref: Preference,
    name: Option<&str>,
    power_env: Option<&str>,
) -> Result<wgpu::Adapter, String> {
    let usable: Vec<&wgpu::Adapter> = adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .collect();
    if usable.is_empty() {
        return Err("no GPU adapter can draw to this window".into());
    }
    if let Some(want) = name
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        && let Some(a) = usable
            .iter()
            .find(|a| a.get_info().name.to_ascii_lowercase().contains(&want))
    {
        return Ok((*a).clone());
    }
    let pref = pref.resolved(power_env);
    let best = usable
        .iter()
        .enumerate()
        .max_by_key(|(i, a)| {
            let info = a.get_info();
            // (ties: Vulkan, Metal or DirectX 12 before OpenGL, then the
            // first listed)
            let api = match info.backend {
                wgpu::Backend::Vulkan | wgpu::Backend::Metal | wgpu::Backend::Dx12 => 2,
                wgpu::Backend::Gl => 1,
                _ => 0,
            };
            (
                rank(&info.name, info.device_type, pref),
                api,
                usize::MAX - i,
            )
        })
        .map(|(_, a)| (*a).clone());
    best.ok_or_else(|| "no GPU adapter".into())
}

/// eframe's wgpu setup with this preference.
pub fn configuration(pref: Preference) -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
        let name = std::env::var("WGPU_ADAPTER_NAME").ok();
        let power = std::env::var("WGPU_POWER_PREF").ok();
        choose(adapters, surface, pref, name.as_deref(), power.as_deref())
    }));
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// "Vulkan · NVIDIA GeForce …".
pub fn describe(info: &wgpu::AdapterInfo) -> String {
    format!("{:?} · {}", info.backend, info.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType as D;

    #[test]
    fn a_dedicated_gpu_first_a_software_one_last() {
        let p = Preference::HighPerformance;
        assert!(rank("NVIDIA RTX", D::DiscreteGpu, p) > rank("Intel Iris", D::IntegratedGpu, p));
        assert!(rank("Intel Iris", D::IntegratedGpu, p) > rank("llvmpipe (LLVM 15)", D::Cpu, p));
        // a software renderer that calls itself something else
        assert_eq!(rank("llvmpipe (LLVM 15.0.6, 256 bits)", D::Other, p), 0);
        assert_eq!(
            rank("Microsoft Basic Render Driver", D::IntegratedGpu, p),
            0
        );
        let low = Preference::LowPower;
        assert!(
            rank("Intel Iris", D::IntegratedGpu, low) > rank("NVIDIA RTX", D::DiscreteGpu, low)
        );
        assert!(rank("NVIDIA RTX", D::DiscreteGpu, low) > rank("lavapipe", D::Cpu, low));
    }

    #[test]
    fn auto_follows_the_environment() {
        assert_eq!(Preference::Auto.resolved(None), Preference::HighPerformance);
        assert_eq!(Preference::Auto.resolved(Some("low")), Preference::LowPower);
        assert_eq!(
            Preference::Auto.resolved(Some("high")),
            Preference::HighPerformance
        );
        assert_eq!(
            Preference::LowPower.resolved(Some("high")),
            Preference::LowPower
        );
        for p in [
            Preference::Auto,
            Preference::HighPerformance,
            Preference::LowPower,
        ] {
            assert_eq!(Preference::from_key(p.key()), p);
        }
        assert_eq!(Preference::from_key("nonsense"), Preference::Auto);
    }

    #[test]
    fn software_renderers_are_known() {
        assert!(is_software("llvmpipe (LLVM 15.0.6, 256 bits)", D::Cpu));
        assert!(is_software("SwiftShader Device", D::Other));
        assert!(!is_software("AMD Radeon RX 7800 XT", D::DiscreteGpu));
    }
}
