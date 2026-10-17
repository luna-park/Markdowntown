//! Выбор графического адаптера.
//!
//! По умолчанию egui-wgpu берёт «высокопроизводительный» адаптер, а Vulkan перечисляется
//! первым — на ноутбуках с двумя видеокартами это Vulkan на дискретной NVIDIA с выводом
//! через встроенную. Со старыми драйверами такое падает прямо в драйвере, без ошибки,
//! которую можно перехватить. Поэтому: DX12 (на Windows надёжнее); среди адаптеров одного
//! API — видеокарта основного монитора, затем других мониторов (иначе каждый кадр
//! копируется на неё с другой видеокарты: у ноутбука экран обычно на встроенной,
//! у настольного компьютера монитор чаще на дискретной; окно открывается обычно на
//! основном мониторе); при равенстве — встроенная раньше дискретной (редактору её
//! хватает); затем Vulkan, OpenGL и в последнюю очередь программный WARP.
//!
//! `WGPU_BACKEND` (`dx12`, `vulkan`, `gl`) по-прежнему ограничивает выбор — для проверки
//! на проблемной машине.

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;

/// Настройки wgpu с нашим выбором адаптера.
pub fn wgpu_options() -> WgpuConfiguration {
    // Всё по умолчанию (в т.ч. учёт `WGPU_BACKEND`), кроме выбора адаптера.
    let mut config = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(WgpuSetupCreateNew {
        native_adapter_selector,
        ..
    }) = &mut config.wgpu_setup
    {
        *native_adapter_selector = Some(Arc::new(select_adapter));
    }
    config
}

/// Монитор рабочего стола: имя видеокарты, к которой он подключён.
struct Display {
    adapter: String,
    primary: bool,
}

/// Что выводит адаптер. Порядок вариантов — порядок предпочтения.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Output {
    PrimaryDisplay,
    OtherDisplay,
    Nothing,
}

/// Чем меньше, тем лучше. Адаптеры, не умеющие выводить в окно, отсеяны заранее.
fn rank(backend: wgpu::Backend, device_type: wgpu::DeviceType, output: Output) -> (u8, Output, u8) {
    use wgpu::{Backend, DeviceType};
    if device_type == DeviceType::Cpu {
        return (9, Output::Nothing, 0);
    }
    let backend = match backend {
        Backend::Dx12 => 0,
        Backend::Vulkan => 1,
        Backend::Gl => 2,
        _ => 3,
    };
    let device = match device_type {
        DeviceType::IntegratedGpu => 0,
        DeviceType::DiscreteGpu => 1,
        DeviceType::VirtualGpu => 2,
        DeviceType::Other => 3,
        DeviceType::Cpu => 4,
    };
    (backend, output, device)
}

/// Что выводит адаптер с именем `adapter` (как его называет wgpu). У OpenGL к имени
/// добавлен хвост (`NVIDIA GeForce RTX 3060/PCIe/SSE2`), поэтому сравнивается начало.
fn output(adapter: &str, displays: &[Display]) -> Output {
    let adapter = adapter.trim().to_lowercase();
    let mut result = Output::Nothing;
    for display in displays {
        let name = display.adapter.trim().to_lowercase();
        if !name.is_empty() && adapter.starts_with(&name) {
            let this = if display.primary {
                Output::PrimaryDisplay
            } else {
                Output::OtherDisplay
            };
            result = result.min(this);
        }
    }
    result
}

/// Мониторы рабочего стола и видеокарты, к которым они подключены.
#[cfg(windows)]
fn displays() -> Vec<Display> {
    use windows_sys::Win32::Graphics::Gdi::{
        DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW,
        EnumDisplayDevicesW,
    };
    let mut displays = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `device` — структура нужного размера (`cb`), имя устройства не задано —
        // перечисляются выходы видеоадаптеров.
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0 {
            let len = device
                .DeviceString
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(device.DeviceString.len());
            displays.push(Display {
                adapter: String::from_utf16_lossy(&device.DeviceString[..len]),
                primary: device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
            });
        }
    }
    displays
}

#[cfg(not(windows))]
fn displays() -> Vec<Display> {
    Vec::new()
}

fn describe(info: &wgpu::AdapterInfo) -> String {
    let driver = [info.driver.as_str(), info.driver_info.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} ({:?}, {:?}, driver {driver})",
        info.name, info.backend, info.device_type
    )
}

fn select_adapter(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    let displays = displays();
    let chosen = adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .min_by_key(|a| {
            let info = a.get_info();
            rank(
                info.backend,
                info.device_type,
                output(&info.name, &displays),
            )
        })
        .cloned()
        .ok_or_else(|| "no graphics adapter can draw to the window".to_owned())?;
    // До создания устройства: если упадёт драйвер, в журнале будет видно, какой.
    let info = chosen.get_info();
    let monitors: Vec<String> = displays
        .iter()
        .map(|d| {
            let primary = if d.primary { " (primary)" } else { "" };
            format!("{}{primary}", d.adapter)
        })
        .collect();
    log::info!(
        "selected graphics adapter: {}, output: {:?}; displays are connected to: {}",
        describe(&info),
        output(&info.name, &displays),
        if monitors.is_empty() {
            "unknown".to_owned()
        } else {
            monitors.join(", ")
        }
    );
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backend, DeviceType};

    /// Порядок адаптеров по `rank`; `displays` — (видеокарта монитора, основной ли он).
    fn order(
        candidates: &[(&'static str, Backend, DeviceType)],
        displays: &[(&str, bool)],
    ) -> Vec<&'static str> {
        let displays: Vec<Display> = displays
            .iter()
            .map(|&(adapter, primary)| Display {
                adapter: adapter.to_owned(),
                primary,
            })
            .collect();
        let mut sorted = candidates.to_vec();
        sorted
            .sort_by_key(|&(name, backend, device)| rank(backend, device, output(name, &displays)));
        sorted.iter().map(|(n, _, _)| *n).collect()
    }

    const NVIDIA: &str = "NVIDIA GeForce RTX 3060";
    const INTEL: &str = "Intel(R) UHD Graphics 770";

    const DESKTOP: [(&str, Backend, DeviceType); 3] = [
        (INTEL, Backend::Dx12, DeviceType::IntegratedGpu),
        (NVIDIA, Backend::Dx12, DeviceType::DiscreteGpu),
        (NVIDIA, Backend::Vulkan, DeviceType::DiscreteGpu),
    ];

    #[test]
    fn laptop_prefers_dx12_integrated_then_vulkan_then_gl_then_warp() {
        // Проблемный ноутбук (AMD + NVIDIA, старые драйверы): экран подключён к AMD.
        // Без сведений о мониторах — тот же порядок.
        let laptop = [
            (
                "NVIDIA GeForce GTX 1650",
                Backend::Vulkan,
                DeviceType::DiscreteGpu,
            ),
            (
                "AMD Radeon Graphics",
                Backend::Vulkan,
                DeviceType::IntegratedGpu,
            ),
            (
                "AMD Radeon Graphics",
                Backend::Dx12,
                DeviceType::IntegratedGpu,
            ),
            (
                "NVIDIA GeForce GTX 1650",
                Backend::Dx12,
                DeviceType::DiscreteGpu,
            ),
            (
                "Microsoft Basic Render Driver",
                Backend::Dx12,
                DeviceType::Cpu,
            ),
            (
                "AMD Radeon Graphics/PCIe/SSE2",
                Backend::Gl,
                DeviceType::Other,
            ),
        ];
        for displays in [&[("AMD Radeon Graphics", true)][..], &[]] {
            let order = order(&laptop, displays);
            assert_eq!(order[0], "AMD Radeon Graphics");
            assert_eq!(order[1], "NVIDIA GeForce GTX 1650");
            assert_eq!(order[5], "Microsoft Basic Render Driver");
        }
    }

    #[test]
    fn desktop_prefers_the_card_with_the_monitor() {
        assert_eq!(order(&DESKTOP, &[(NVIDIA, true)]), [NVIDIA, INTEL, NVIDIA]);
        // Несколько мониторов на одной видеокарте — она же.
        assert_eq!(
            order(&DESKTOP, &[(NVIDIA, true), (NVIDIA, false)])[0],
            NVIDIA
        );
    }

    #[test]
    fn monitors_on_both_cards_prefer_the_primary_monitor() {
        assert_eq!(
            order(&DESKTOP, &[(INTEL, false), (NVIDIA, true)])[0],
            NVIDIA
        );
        assert_eq!(order(&DESKTOP, &[(INTEL, true), (NVIDIA, false)])[0], INTEL);
        // Основной неизвестен (не нашёлся среди адаптеров) — встроенная раньше.
        let unknown = [
            (INTEL, false),
            (NVIDIA, false),
            ("DisplayLink USB Device", true),
        ];
        assert_eq!(order(&DESKTOP, &unknown)[0], INTEL);
    }

    #[test]
    fn display_names_match_adapter_names() {
        let displays = [Display {
            adapter: NVIDIA.to_owned(),
            primary: true,
        }];
        assert_eq!(output(NVIDIA, &displays), Output::PrimaryDisplay);
        assert_eq!(
            output("nvidia geforce rtx 3060/PCIe/SSE2", &displays),
            Output::PrimaryDisplay
        );
        assert_eq!(output(INTEL, &displays), Output::Nothing);
        let empty = [Display {
            adapter: String::new(),
            primary: true,
        }];
        assert_eq!(output(NVIDIA, &empty), Output::Nothing);
    }
}
