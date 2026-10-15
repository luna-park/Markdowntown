//! Выбор графического адаптера.
//!
//! По умолчанию egui-wgpu берёт «высокопроизводительный» адаптер, а Vulkan перечисляется
//! первым — на ноутбуках с двумя видеокартами это Vulkan на дискретной NVIDIA с выводом
//! через встроенную. Со старыми драйверами такое падает прямо в драйвере, без ошибки,
//! которую можно перехватить. Поэтому: DX12 (на Windows надёжнее), встроенная видеокарта
//! раньше дискретной (редактору её хватает, без передачи кадров между видеокартами),
//! затем Vulkan, OpenGL и в последнюю очередь программный WARP.
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

/// Чем меньше, тем лучше. Адаптеры, не умеющие выводить в окно, отсеяны заранее.
fn rank(backend: wgpu::Backend, device_type: wgpu::DeviceType) -> (u8, u8) {
    use wgpu::{Backend, DeviceType};
    if device_type == DeviceType::Cpu {
        return (9, 0);
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
    (backend, device)
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
    let chosen = adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .min_by_key(|a| {
            let info = a.get_info();
            rank(info.backend, info.device_type)
        })
        .cloned()
        .ok_or_else(|| "no graphics adapter can draw to the window".to_owned())?;
    // До создания устройства: если упадёт драйвер, в журнале будет видно, какой.
    log::info!(
        "selected graphics adapter: {}",
        describe(&chosen.get_info())
    );
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backend, DeviceType};

    #[test]
    fn prefers_dx12_integrated_then_vulkan_then_gl_then_warp() {
        // Набор адаптеров с проблемного ноутбука (AMD + NVIDIA, старые драйверы).
        let mut candidates = [
            ("vulkan nvidia", Backend::Vulkan, DeviceType::DiscreteGpu),
            ("vulkan amd", Backend::Vulkan, DeviceType::IntegratedGpu),
            ("dx12 amd", Backend::Dx12, DeviceType::IntegratedGpu),
            ("dx12 nvidia", Backend::Dx12, DeviceType::DiscreteGpu),
            ("warp", Backend::Dx12, DeviceType::Cpu),
            ("gl amd", Backend::Gl, DeviceType::Other),
        ];
        candidates.sort_by_key(|&(_, backend, device)| rank(backend, device));
        let order: Vec<_> = candidates.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(
            order,
            [
                "dx12 amd",
                "dx12 nvidia",
                "vulkan amd",
                "vulkan nvidia",
                "gl amd",
                "warp"
            ]
        );
    }
}
