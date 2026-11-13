pub mod state;
pub mod tray;
pub mod views;

use anyhow::Result;
use eframe::egui;
use pac_core::config::PortfolioConfig;
use state::{BackgroundEvent, GuiTab, PacSentinelGuiApp};
use std::path::PathBuf;
use tray::{AppTray, TrayAction};

struct GuiWrapper {
    app: PacSentinelGuiApp,
    tray: Option<AppTray>,
}

impl eframe::App for GuiWrapper {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Riceve eventi completati dai worker asincroni Tokio
        while let Ok(ev) = self.app.bg_rx.try_recv() {
            match ev {
                BackgroundEvent::MarketRefreshed(res) => {
                    self.app.is_busy = false;
                    match res {
                        Ok(msg) => {
                            let _ = self.app.reload_from_db();
                            self.app.status_banner = msg;
                            if let Some(ref tray) = self.tray {
                                tray.update_level(self.app.report.overall_level);
                            }
                        }
                        Err(err) => {
                            self.app.status_banner = format!("Errore aggiornamento dati: {err}");
                        }
                    }
                }
                BackgroundEvent::TelegramTestSent(res) => match res {
                    Ok(msg) => self.app.status_banner = msg,
                    Err(err) => self.app.status_banner = format!("Errore Telegram: {err}"),
                },
            }
        }

        // 2. Gestisce eventuali azioni dall'icona nella barra di sistema
        if let Some(ref tray) = self.tray {
            if let Some(action) = tray.poll_action() {
                match action {
                    TrayAction::OpenWindow => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::RefreshNow => {
                        self.app.trigger_market_refresh();
                    }
                    TrayAction::Quit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        views::render_top_header(ctx, &mut self.app);
        views::render_bottom_footer(ctx, &self.app);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.app.active_tab {
                GuiTab::Panoramica => views::render_panoramica_tab(ui, &self.app),
                GuiTab::PianoPac => views::render_piano_pac_tab(ui, &mut self.app),
                GuiTab::Operazioni => views::render_operazioni_tab(ui, &mut self.app),
                GuiTab::Simulatore => views::render_simulatore_tab(ui, &mut self.app),
                GuiTab::TesiDiario => views::render_tesi_diario_tab(ui, &mut self.app),
                GuiTab::Impostazioni => views::render_impostazioni_tab(ui, &mut self.app),
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

pub fn launch_desktop_gui(
    config_path: PathBuf,
    db_path: PathBuf,
    config: PortfolioConfig,
    rt_handle: tokio::runtime::Handle,
) -> Result<()> {
    let app_state = PacSentinelGuiApp::new(config_path, db_path, config, rt_handle)?;
    let tray = AppTray::try_init(app_state.report.overall_level);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PAC Sentinel - Gestione Portafoglio")
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([920.0, 620.0]),
        ..Default::default()
    };

    eframe::run_native(
        "PAC Sentinel",
        native_options,
        Box::new(move |_cc| Ok(Box::new(GuiWrapper { app: app_state, tray }))),
    )
    .map_err(|e| anyhow::anyhow!("Errore avvio finestra grafica eframe: {e}"))?;

    Ok(())
}