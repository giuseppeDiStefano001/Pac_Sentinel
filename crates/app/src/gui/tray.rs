use pac_core::types::SignalLevel;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenWindow,
    RefreshNow,
    Quit,
}

pub struct AppTray {
    tray: TrayIcon,
    open_id: tray_icon::menu::MenuId,
    refresh_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl AppTray {
    pub fn try_init(initial_level: SignalLevel) -> Option<Self> {
        let menu = Menu::new();
        let open_item = MenuItem::new("Apri PAC Sentinel", true, None);
        let refresh_item = MenuItem::new("Aggiorna quotazioni ora", true, None);
        let quit_item = MenuItem::new("Esci", true, None);

        let open_id = open_item.id().clone();
        let refresh_id = refresh_item.id().clone();
        let quit_id = quit_item.id().clone();

        let _ = menu.append(&open_item);
        let _ = menu.append(&refresh_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit_item);

        let icon = build_badge_icon(initial_level)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("PAC Sentinel - Portafoglio lungo termine")
            .with_icon(icon)
            .build()
            .ok()?;

        Some(Self {
            tray,
            open_id,
            refresh_id,
            quit_id,
        })
    }

    pub fn update_level(&self, level: SignalLevel) {
        if let Some(icon) = build_badge_icon(level) {
            let _ = self.tray.set_icon(Some(icon));
        }
    }

    pub fn poll_action(&self) -> Option<TrayAction> {
        if let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.open_id {
                return Some(TrayAction::OpenWindow);
            }
            if ev.id == self.refresh_id {
                return Some(TrayAction::RefreshNow);
            }
            if ev.id == self.quit_id {
                return Some(TrayAction::Quit);
            }
        }
        None
    }
}

fn build_badge_icon(level: SignalLevel) -> Option<Icon> {
    let (r, g, b) = match level {
        SignalLevel::Verde => (34_u8, 197_u8, 94_u8),
        SignalLevel::Giallo => (234_u8, 179_u8, 8_u8),
        SignalLevel::Arancione => (249_u8, 115_u8, 22_u8),
        SignalLevel::Rosso => (239_u8, 68_u8, 68_u8),
    };
    let size = 16_u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let is_border = x == 0 || y == 0 || x == size - 1 || y == size - 1;
            if is_border {
                rgba.extend_from_slice(&[20, 24, 31, 255]);
            } else {
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, size, size).ok()
}