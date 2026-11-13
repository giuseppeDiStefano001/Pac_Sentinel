use crate::gui::state::{dec_to_f64, GuiTab, PacSentinelGuiApp};
use crate::secrets;
use chrono::NaiveDate;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints};
use pac_core::pac::calculate_base_pac_allocation;
use pac_core::signals::phase_label;
use pac_core::simulator::{
    run_pac_simulation, AnnualReturnAssumptions, ScenarioKind, SCENARIO_DISCLAIMER,
};
use pac_core::types::{AssetId, Currency, SignalLevel};
use pac_core::LEGAL_DISCLAIMER;
use pac_data::db::PortfolioRepository;
use pac_data::market_hours::now_rome;
use pac_data::providers::bce::BceFxClient;
use pac_data::providers::FallbackPriceChain;
use pac_data::service::MarketDataService;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::str::FromStr;

pub fn signal_color(level: SignalLevel) -> Color32 {
    match level {
        SignalLevel::Verde => Color32::from_rgb(34, 197, 94),
        SignalLevel::Giallo => Color32::from_rgb(234, 179, 8),
        SignalLevel::Arancione => Color32::from_rgb(249, 115, 22),
        SignalLevel::Rosso => Color32::from_rgb(239, 68, 68),
    }
}

pub fn render_top_header(ctx: &egui::Context, app: &mut PacSentinelGuiApp) {
    egui::TopBottomPanel::top("top_header").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("PAC Sentinel");
            ui.separator();

            let lvl = app.report.overall_level;
            ui.label(
                RichText::new(format!("Semaforo: {lvl}"))
                    .strong()
                    .color(signal_color(lvl)),
            );
            ui.separator();
            ui.label(format!("Cambio BCE EUR/USD: {}", app.fx_date_label));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let btn_label = if app.is_busy {
                    "Aggiornamento..."
                } else {
                    "Aggiorna quotazioni ora"
                };
                if ui
                    .add_enabled(!app.is_busy, egui::Button::new(btn_label))
                    .clicked()
                {
                    app.trigger_market_refresh();
                }
            });
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut app.active_tab, GuiTab::Panoramica, "1. Panoramica");
            ui.selectable_value(&mut app.active_tab, GuiTab::PianoPac, "2. Piano PAC");
            ui.selectable_value(&mut app.active_tab, GuiTab::Operazioni, "3. Operazioni");
            ui.selectable_value(&mut app.active_tab, GuiTab::Simulatore, "4. Simulatore & Vendita");
            ui.selectable_value(&mut app.active_tab, GuiTab::TesiDiario, "5. Tesi & Diario");
            ui.selectable_value(&mut app.active_tab, GuiTab::Impostazioni, "6. Impostazioni & Telegram");
        });
        ui.add_space(4.0);
    });
}

pub fn render_bottom_footer(ctx: &egui::Context, app: &PacSentinelGuiApp) {
    egui::TopBottomPanel::bottom("bottom_footer").show(ctx, |ui| {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&app.status_banner).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Fondo emergenze TR: {:.2} EUR (INTOCCABILE)",
                        app.ef_status.balance_eur
                    ))
                    .small()
                    .strong()
                    .color(Color32::from_rgb(96, 165, 250)),
                );
            });
        });
        ui.label(
            RichText::new(LEGAL_DISCLAIMER)
                .small()
                .italics()
                .color(Color32::GRAY),
        );
        ui.add_space(3.0);
    });
}

pub fn render_panoramica_tab(ui: &mut egui::Ui, app: &PacSentinelGuiApp) {
    render_portfolio_summary_card(ui, app);
    ui.add_space(8.0);
    render_asset_cards_row(ui, app);

    for alert in &app.report.portfolio_alerts {
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(249, 115, 22), format!("! {alert}"));
    }

    ui.add_space(8.0);
    render_weights_bar_chart(ui, app);
}

fn render_portfolio_summary_card(ui: &mut egui::Ui, app: &PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Riepilogo Portafoglio Investito (escluso Fondo Emergenze)");
        ui.columns(4, |cols| {
            cols[0].label(RichText::new("Valore Mercato Attuale").small().color(Color32::GRAY));
            cols[0].label(
                RichText::new(format!("{:.2} EUR", app.valuation.total_market_value_eur))
                    .size(20.0)
                    .strong(),
            );
            cols[0].label(format!("Investito: {:.2} EUR", app.valuation.total_invested_eur));

            cols[1].label(RichText::new("P&L Netto (post 26%)").small().color(Color32::GRAY));
            let pnl_col = if app.valuation.total_net_pnl_eur >= Decimal::ZERO {
                Color32::from_rgb(34, 197, 94)
            } else {
                Color32::from_rgb(239, 68, 68)
            };
            cols[1].label(
                RichText::new(format!("{:+.2} EUR", app.valuation.total_net_pnl_eur))
                    .size(20.0)
                    .strong()
                    .color(pnl_col),
            );
            let est_tax = app.valuation.total_gross_pnl_eur - app.valuation.total_net_pnl_eur;
            cols[1].label(format!(
                "Lordo: {:+.2} EUR (Tasse: {:.2} EUR)",
                app.valuation.total_gross_pnl_eur, est_tax
            ));

            cols[2].label(RichText::new("Liquidità TR PAC Residua").small().color(Color32::GRAY));
            cols[2].label(
                RichText::new(format!("{:.2} EUR", app.tr_cash_remaining))
                    .size(20.0)
                    .strong(),
            );
            cols[2].label(format!(
                "Mese {} | {} rate Fase 1 coperte",
                app.current_pac_month, app.covered_months
            ));

            cols[3].label(RichText::new("Esposizione Effettiva").small().color(Color32::GRAY));
            cols[3].label(
                RichText::new(format!(
                    "NVDA eff.: {:.2}%",
                    app.valuation.effective_nvda_exposure_pct
                ))
                .size(18.0)
                .strong(),
            );
            cols[3].label(format!(
                "Satelliti totali: {:.2}%",
                app.valuation.satellites_weight_pct
            ));
        });

        if app.ef_status.is_above_warning_threshold {
            ui.add_space(4.0);
            ui.colored_label(
                Color32::from_rgb(234, 179, 8),
                format!(
                    "ATTENZIONE IMPOSTA DI BOLLO: il fondo emergenze ({:.2} EUR) supera {:.2} EUR (soglia legale {:.2} EUR).",
                    app.ef_status.balance_eur,
                    app.ef_status.stamp_duty_warning_threshold_eur,
                    app.ef_status.stamp_duty_legal_threshold_eur
                ),
            );
        }
    });
}

fn render_asset_cards_row(ui: &mut egui::Ui, app: &PacSentinelGuiApp) {
    ui.columns(3, |cols| {
        for (idx, asset) in AssetId::ALL.iter().enumerate() {
            let ui_col = &mut cols[idx];
            let sig = &app.report.asset_signals[asset];
            let pos = &app.valuation.positions[asset];
            ui_col.group(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(asset.code());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("[{}]", sig.level))
                                .strong()
                                .color(signal_color(sig.level)),
                        );
                    });
                });
                ui.label(RichText::new(asset.display_name()).small().color(Color32::GRAY));
                ui.separator();

                if let Some(s) = app.snapshots.get(asset) {
                    ui.label(
                        RichText::new(format!(
                            "{:.2} {} (~{:.2} EUR)",
                            s.price_native, s.currency, s.price_eur
                        ))
                        .size(16.0)
                        .strong(),
                    );
                    ui.label(format!(
                        "Var 7g: {:+.2}% | Var 30g: {:+.2}% | DD 52w: {:.2}%",
                        s.change_7d_pct,
                        s.change_30d_pct,
                        s.drawdown_52w_pct()
                    ));
                    let stale_txt = if s.is_stale { " [DATI VECCHI]" } else { "" };
                    ui.label(
                        RichText::new(format!(
                            "Fonte: {} ({}){stale_txt}",
                            s.source,
                            s.timestamp_rome.format("%d/%m %H:%M")
                        ))
                        .small()
                        .color(Color32::GRAY),
                    );
                } else {
                    ui.label("Quotazione non ancora scaricata");
                }

                ui.separator();
                ui.label(format!(
                    "Peso Reale: {:.2}% | Atteso: {:.2}% | Target: {:.2}%",
                    pos.actual_weight_pct, pos.expected_weight_pct, pos.target_weight_pct
                ));
                ui.label(format!(
                    "Valore: {:.2} EUR (P&L netto: {:+.2} EUR)",
                    pos.current_market_value_eur, pos.unrealized_net_pnl_eur
                ));
                ui.add_space(4.0);
                ui.label(RichText::new("Regola:").strong());
                ui.label(&sig.triggered_rule);
                ui.label(RichText::new("Azione suggerita:").strong());
                ui.label(&sig.suggested_pac_action);
            });
        }
    });
}

fn render_weights_bar_chart(ui: &mut egui::Ui, app: &PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Confronto Pesi Portafoglio (%): Reale vs Atteso al Mese vs Target");
        let mut real_bars = Vec::new();
        let mut exp_bars = Vec::new();
        let mut tgt_bars = Vec::new();

        for (i, asset) in AssetId::ALL.iter().enumerate() {
            let p = &app.valuation.positions[asset];
            let base_x = Decimal::from(u32::try_from(i).unwrap_or(0)) * dec!(3);
            real_bars.push(Bar::new(dec_to_f64(base_x), dec_to_f64(p.actual_weight_pct)).width(0.7));
            exp_bars.push(
                Bar::new(dec_to_f64(base_x + dec!(0.75)), dec_to_f64(p.expected_weight_pct))
                    .width(0.7),
            );
            tgt_bars.push(
                Bar::new(dec_to_f64(base_x + dec!(1.50)), dec_to_f64(p.target_weight_pct))
                    .width(0.7),
            );
        }

        Plot::new("weights_plot")
            .height(210.0)
            .legend(Legend::default())
            .x_axis_formatter(|mark, _range| {
                let Some(dec_val) = Decimal::try_from(mark.value).ok() else {
                    return String::new();
                };
                if dec_val.fract().abs() > dec!(0.05) {
                    return String::new();
                }
                match dec_val.round().to_i64().unwrap_or(-1) {
                    1 => "ETF S&P 500".to_string(),
                    4 => "Nvidia (NVDA)".to_string(),
                    7 => "MP Materials (MP)".to_string(),
                    _ => String::new(),
                }
            })
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(
                    BarChart::new(real_bars)
                        .name("Peso Reale %")
                        .color(Color32::from_rgb(59, 130, 246)),
                );
                plot_ui.bar_chart(
                    BarChart::new(exp_bars)
                        .name("Peso Atteso al Mese %")
                        .color(Color32::from_rgb(234, 179, 8)),
                );
                plot_ui.bar_chart(
                    BarChart::new(tgt_bars)
                        .name("Peso Target 80/15/5 %")
                        .color(Color32::from_rgb(34, 197, 94)),
                );
            });
    });
}

pub fn render_piano_pac_tab(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Configurazione Mese Corrente & Piano PAC");
        ui.horizontal(|ui| {
            ui.label("Mese corrente del piano (1..120):");
            let mut m = app.current_pac_month;
            if ui.add(egui::DragValue::new(&mut m).range(1..=120)).changed() {
                app.current_pac_month = m;
                if let Ok(repo) = PortfolioRepository::open_file(&app.db_path) {
                    let _ = repo.set_current_pac_month(m);
                }
                let _ = app.reload_from_db();
            }
            ui.separator();
            ui.label(format!(
                "Fase attiva: {}",
                phase_label(app.report.adjusted_pac_allocation.phase)
            ));
        });
    });

    ui.add_space(8.0);

    ui.columns(2, |cols| {
        cols[0].group(|ui| {
            ui.heading("Rata Base da Piano (80 / 15 / 5)");
            if let Ok(base) = calculate_base_pac_allocation(&app.config, app.current_pac_month) {
                ui.label(format!("Totale rata: {:.2} EUR", base.total_eur));
                ui.label(format!("• ETF S&P 500: {:.2} EUR", base.etf_sp500_eur));
                ui.label(format!("• Nvidia (NVDA): {:.2} EUR", base.nvda_eur));
                ui.label(format!("• MP Materials (MP): {:.2} EUR", base.mp_eur));
            }
        });

        cols[1].group(|ui| {
            ui.heading("Rata Suggerita dal Motore Regole");
            let adj = &app.report.adjusted_pac_allocation;
            ui.label(RichText::new(format!("Totale rata: {:.2} EUR", adj.total_eur)).strong());
            ui.label(format!("• ETF S&P 500: {:.2} EUR", adj.etf_sp500_eur));
            ui.label(format!("• Nvidia (NVDA): {:.2} EUR", adj.nvda_eur));
            ui.label(format!("• MP Materials (MP): {:.2} EUR", adj.mp_eur));
        });
    });

    ui.add_space(8.0);

    ui.group(|ui| {
        ui.heading("Date Prossime Trimestrali (Promemoria il giorno prima)");
        ui.horizontal(|ui| {
            ui.label("Trimestrale NVDA (YYYY-MM-DD):");
            ui.text_edit_singleline(&mut app.nvda_earnings_str);
            ui.label("Trimestrale MP (YYYY-MM-DD):");
            ui.text_edit_singleline(&mut app.mp_earnings_str);
            if ui.button("Salva date trimestrali").clicked() {
                if let Ok(repo) = PortfolioRepository::open_file(&app.db_path) {
                    if let Ok(d) = NaiveDate::parse_from_str(app.nvda_earnings_str.trim(), "%Y-%m-%d")
                    {
                        let _ = repo.set_earnings_date(AssetId::Nvda, d);
                    }
                    if let Ok(d) = NaiveDate::parse_from_str(app.mp_earnings_str.trim(), "%Y-%m-%d")
                    {
                        let _ = repo.set_earnings_date(AssetId::Mp, d);
                    }
                    let _ = app.reload_from_db();
                    app.status_banner = "Date trimestrali aggiornate.".to_string();
                }
            }
        });
    });
}

pub fn render_operazioni_tab(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Registra Nuova Operazione di Acquisto");
        ui.horizontal(|ui| {
            ui.label("Strumento:");
            egui::ComboBox::from_id_salt("new_op_asset")
                .selected_text(app.new_op_form.asset.code())
                .show_ui(ui, |ui| {
                    for a in AssetId::ALL {
                        ui.selectable_value(&mut app.new_op_form.asset, a, a.display_name());
                    }
                });

            ui.label("Data (YYYY-MM-DD):");
            ui.add(egui::TextEdit::singleline(&mut app.new_op_form.date_str).desired_width(95.0));

            ui.label("Investito EUR:");
            ui.add(
                egui::TextEdit::singleline(&mut app.new_op_form.invested_eur_str).desired_width(75.0),
            );

            ui.label("Prezzo unitario nativo:");
            ui.add(
                egui::TextEdit::singleline(&mut app.new_op_form.unit_price_str).desired_width(75.0),
            );

            ui.label("EUR/USD:");
            ui.add(egui::TextEdit::singleline(&mut app.new_op_form.eur_usd_str).desired_width(65.0));
        });

        ui.horizontal(|ui| {
            ui.checkbox(
                &mut app.new_op_form.is_extra_advance,
                "Tranche extra di anticipo ETF su drawdown (scala dalla liquidità TR)",
            );
            ui.label("Nota:");
            ui.add(egui::TextEdit::singleline(&mut app.new_op_form.note_str).desired_width(220.0));

            if ui.button("Registra acquisto").clicked() {
                if let Err(err) = app.insert_operation_from_form() {
                    app.status_banner = format!("Errore inserimento operazione: {err}");
                }
            }
        });
    });

    ui.add_space(8.0);

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.heading(format!("Registro Operazioni ({})", app.operations.len()));
            if ui.button("Esporta CSV").clicked() {
                let export_path = app.config_path.with_file_name("operazioni_pac.csv");
                if let Ok(mut wtr) = csv::Writer::from_path(&export_path) {
                    let _ = wtr.write_record([
                        "id",
                        "data",
                        "strumento",
                        "investito_eur",
                        "prezzo_nativo",
                        "valuta",
                        "eur_usd",
                        "quantita",
                        "nota",
                    ]);
                    for op in &app.operations {
                        let _ = wtr.write_record([
                            op.id.unwrap_or(0).to_string(),
                            op.date.to_string(),
                            op.asset.code().to_string(),
                            op.invested_eur.to_string(),
                            op.unit_price_native.to_string(),
                            op.native_currency.to_string(),
                            op.eur_usd_rate.to_string(),
                            op.quantity.to_string(),
                            op.note.clone().unwrap_or_default(),
                        ]);
                    }
                    let _ = wtr.flush();
                    app.status_banner = format!("Esportato CSV in {}", export_path.display());
                }
            }
        });

        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("ops_grid")
                .striped(true)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("Data");
                    ui.strong("Strumento");
                    ui.strong("Investito EUR");
                    ui.strong("Prezzo Nativo");
                    ui.strong("EUR/USD");
                    ui.strong("Quantità");
                    ui.strong("Nota");
                    ui.end_row();

                    for op in &app.operations {
                        ui.label(op.date.to_string());
                        ui.label(op.asset.code());
                        ui.label(format!("{:.2} EUR", op.invested_eur));
                        ui.label(format!("{} {}", op.unit_price_native, op.native_currency));
                        ui.label(op.eur_usd_rate.to_string());
                        ui.label(op.quantity.to_string());
                        ui.label(op.note.as_deref().unwrap_or("-"));
                        ui.end_row();
                    }
                });
        });
    });
}

pub fn render_simulatore_tab(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    render_pac_projection_section(ui, app);
    ui.add_space(8.0);
    render_hypothetical_sale_section(ui, app);
}

fn build_custom_scenarios(app: &PacSentinelGuiApp) -> [AnnualReturnAssumptions; 3] {
    let c_pct = Decimal::from_str(app.sim_cautious_pct.trim()).unwrap_or(Decimal::from(3));
    let b_pct = Decimal::from_str(app.sim_base_pct.trim()).unwrap_or(Decimal::from(7));
    let o_pct = Decimal::from_str(app.sim_optimistic_pct.trim()).unwrap_or(Decimal::from(10));
    [
        AnnualReturnAssumptions {
            kind: ScenarioKind::Prudente,
            label: "Prudente (ipotesi, non previsione)",
            etf_annual_pct: c_pct,
            nvda_annual_pct: c_pct,
            mp_annual_pct: c_pct,
        },
        AnnualReturnAssumptions {
            kind: ScenarioKind::Base,
            label: "Base (ipotesi, non previsione)",
            etf_annual_pct: b_pct,
            nvda_annual_pct: b_pct + Decimal::from(2),
            mp_annual_pct: b_pct,
        },
        AnnualReturnAssumptions {
            kind: ScenarioKind::Ottimistico,
            label: "Ottimistico (ipotesi, non previsione)",
            etf_annual_pct: o_pct,
            nvda_annual_pct: o_pct + Decimal::from(4),
            mp_annual_pct: o_pct + Decimal::from(2),
        },
    ]
}

fn render_pac_projection_section(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Simulatore PAC & Proiezione 3 Scenari");
        ui.label(
            RichText::new(SCENARIO_DISCLAIMER)
                .small()
                .italics()
                .color(Color32::GRAY),
        );
        ui.horizontal(|ui| {
            ui.label("Orizzonte (anni):");
            ui.add(egui::Slider::new(&mut app.sim_horizon_years, 1..=30));
            ui.label("Prudente %:");
            ui.add(egui::TextEdit::singleline(&mut app.sim_cautious_pct).desired_width(45.0));
            ui.label("Base %:");
            ui.add(egui::TextEdit::singleline(&mut app.sim_base_pct).desired_width(45.0));
            ui.label("Ottimistico %:");
            ui.add(egui::TextEdit::singleline(&mut app.sim_optimistic_pct).desired_width(45.0));
        });

        let months = u32::try_from(app.sim_horizon_years).unwrap_or(10) * 12;
        let scenarios = build_custom_scenarios(app);

        if let (Ok(sim_c), Ok(sim_b), Ok(sim_o)) = (
            run_pac_simulation(&app.config, scenarios[0], months),
            run_pac_simulation(&app.config, scenarios[1], months),
            run_pac_simulation(&app.config, scenarios[2], months),
        ) {
            let inv_pts: PlotPoints = sim_b
                .timeline
                .iter()
                .map(|p| [f64::from(p.month), dec_to_f64(p.cumulative_invested_eur)])
                .collect();
            let c_pts: PlotPoints = sim_c
                .timeline
                .iter()
                .map(|p| [f64::from(p.month), dec_to_f64(p.portfolio_value_eur)])
                .collect();
            let b_pts: PlotPoints = sim_b
                .timeline
                .iter()
                .map(|p| [f64::from(p.month), dec_to_f64(p.portfolio_value_eur)])
                .collect();
            let o_pts: PlotPoints = sim_o
                .timeline
                .iter()
                .map(|p| [f64::from(p.month), dec_to_f64(p.portfolio_value_eur)])
                .collect();

            Plot::new("pac_sim_plot")
                .height(220.0)
                .legend(Legend::default())
                .show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new(inv_pts)
                            .name("Capitale Versato EUR")
                            .color(Color32::GRAY),
                    );
                    plot_ui.line(
                        Line::new(c_pts)
                            .name("Scenario Prudente")
                            .color(Color32::from_rgb(234, 179, 8)),
                    );
                    plot_ui.line(
                        Line::new(b_pts)
                            .name("Scenario Base")
                            .color(Color32::from_rgb(59, 130, 246)),
                    );
                    plot_ui.line(
                        Line::new(o_pts)
                            .name("Scenario Ottimistico")
                            .color(Color32::from_rgb(34, 197, 94)),
                    );
                });

            if let (Some(lc), Some(lb), Some(lo)) = (
                sim_c.timeline.last(),
                sim_b.timeline.last(),
                sim_o.timeline.last(),
            ) {
                ui.label(format!(
                    "Al mese {months}: Versato {:.2} EUR | Prudente: {:.2} EUR | Base: {:.2} EUR | Ottimistico: {:.2} EUR",
                    lb.cumulative_invested_eur,
                    lc.portfolio_value_eur,
                    lb.portfolio_value_eur,
                    lo.portfolio_value_eur
                ));
            }
        }
    });
}

fn render_hypothetical_sale_section(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Simulatore Vendita Ipotetica & Imposta Sostitutiva 26% (Regime Amministrato)");
        ui.horizontal(|ui| {
            ui.label("Strumento:");
            egui::ComboBox::from_id_salt("sale_sim_asset")
                .selected_text(app.sale_sim_form.asset.code())
                .show_ui(ui, |ui| {
                    for a in AssetId::ALL {
                        ui.selectable_value(&mut app.sale_sim_form.asset, a, a.display_name());
                    }
                });

            ui.label("Quantità da vendere:");
            ui.add(
                egui::TextEdit::singleline(&mut app.sale_sim_form.qty_to_sell_str).desired_width(80.0),
            );

            ui.label("Prezzo vendita EUR:");
            ui.add(
                egui::TextEdit::singleline(&mut app.sale_sim_form.sale_price_eur_str)
                    .desired_width(80.0),
            );

            if ui.button("Calcola impatto vendita").clicked() {
                if let Err(err) = app.run_sale_simulation() {
                    app.status_banner = format!("Errore simulazione vendita: {err}");
                }
            }
        });

        if let Some(res) = &app.sale_sim_form.last_result {
            ui.add_space(4.0);
            ui.label(format!(
                "Ricavo lordo: {:.2} EUR | Plusvalenza/Minusvalenza: {:+.2} EUR | Imposta 26% trattenuta: {:.2} EUR | Netto accreditato su TR: {:.2} EUR | Quote residue: {}",
                res.gross_proceeds_eur,
                res.capital_gain_eur,
                res.capital_gains_tax_eur,
                res.net_proceeds_eur,
                res.remaining_quantity
            ));
            ui.label(
                RichText::new(&res.flow_rebalance_comparison)
                    .small()
                    .italics(),
            );
        }
    });
}

pub fn render_tesi_diario_tab(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Checklist Qualitativa Tesi di Investimento (Alimenta i Semafori)");
        ui.checkbox(
            &mut app.thesis_state.nvda_confirmed,
            "Nvidia (NVDA): tesi AI / Data Center confermata dall'ultima trimestrale",
        );
        ui.horizontal(|ui| {
            ui.label("Note tesi NVDA:");
            ui.text_edit_singleline(&mut app.thesis_state.nvda_notes);
        });

        ui.separator();

        ui.checkbox(
            &mut app.thesis_state.mp_confirmed,
            "MP Materials (MP): tesi filiera terre rare USA confermata",
        );
        ui.checkbox(
            &mut app.thesis_state.mp_compromised,
            "MP Materials (MP): TESI COMPROMESSA (se attiva, blocca qualsiasi mediazione su cali >50%)",
        );
        ui.horizontal(|ui| {
            ui.label("Note tesi MP:");
            ui.text_edit_singleline(&mut app.thesis_state.mp_notes);
        });

        if ui.button("Salva stato tesi e ricalcola segnali").clicked() {
            if let Ok(repo) = PortfolioRepository::open_file(&app.db_path) {
                let _ = repo.save_thesis_state(&app.thesis_state, now_rome());
                let _ = app.reload_from_db();
                app.status_banner = "Stato tesi salvato e semafori aggiornati.".to_string();
            }
        }
    });

    ui.add_space(8.0);

    ui.group(|ui| {
        ui.heading("Diario delle Decisioni");
        ui.horizontal(|ui| {
            ui.label("Strumento:");
            egui::ComboBox::from_id_salt("journal_asset")
                .selected_text(app.new_journal_asset.map_or("Portafoglio", AssetId::code))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.new_journal_asset, None, "Portafoglio generale");
                    for a in AssetId::ALL {
                        ui.selectable_value(&mut app.new_journal_asset, Some(a), a.display_name());
                    }
                });
            ui.label("Titolo:");
            ui.text_edit_singleline(&mut app.new_journal_title);
        });
        ui.horizontal(|ui| {
            ui.label("Nota:");
            ui.text_edit_singleline(&mut app.new_journal_body);
            if ui.button("Aggiungi al diario").clicked()
                && !app.new_journal_title.trim().is_empty()
            {
                if let Ok(repo) = PortfolioRepository::open_file(&app.db_path) {
                    let _ = repo.add_journal_entry(
                        now_rome().date_naive(),
                        app.new_journal_asset,
                        app.new_journal_title.trim(),
                        app.new_journal_body.trim(),
                    );
                    app.new_journal_title.clear();
                    app.new_journal_body.clear();
                    let _ = app.reload_from_db();
                    app.status_banner = "Nota aggiunta al diario decisioni.".to_string();
                }
            }
        });

        ui.separator();
        egui::ScrollArea::vertical().max_height(210.0).show(ui, |ui| {
            for entry in &app.journal_entries {
                let tag = entry.asset.map_or("PORTAFOGLIO", AssetId::code);
                ui.label(
                    RichText::new(format!("[{} | {tag}] {}", entry.entry_date, entry.title))
                        .strong(),
                );
                if !entry.body.is_empty() {
                    ui.label(&entry.body);
                }
                ui.separator();
            }
        });
    });
}

pub fn render_impostazioni_tab(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.columns(2, |cols| {
        render_secrets_and_manual_price_col(&mut cols[0], app);
        render_config_editor_col(&mut cols[1], app);
    });
}

fn render_secrets_and_manual_price_col(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Segreti (Keyring OS), Telegram & Prezzo Manuale");
        ui.horizontal(|ui| {
            ui.label("Token Bot Telegram:");
            ui.add(
                egui::TextEdit::singleline(&mut app.settings_form.telegram_token_input)
                    .password(true),
            );
            let token_val = app.settings_form.telegram_token_input.trim().to_string();
            if ui.button("Salva nel Keyring").clicked()
                && !token_val.is_empty()
                && secrets::store_secret("telegram_bot_token", &token_val).is_ok()
            {
                app.settings_form.telegram_token_input.clear();
                app.status_banner = "Token Telegram salvato nel keyring OS.".to_string();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Chiave API Mercato:");
            ui.add(
                egui::TextEdit::singleline(&mut app.settings_form.market_api_key_input)
                    .password(true),
            );
            let key_val = app.settings_form.market_api_key_input.trim().to_string();
            if ui.button("Salva nel Keyring").clicked()
                && !key_val.is_empty()
                && secrets::store_secret("market_api_key", &key_val).is_ok()
            {
                app.settings_form.market_api_key_input.clear();
                app.status_banner = "Chiave API mercato salvata nel keyring OS.".to_string();
            }
        });

        if ui.button("Invia messaggio di prova su Telegram").clicked() {
            app.trigger_telegram_test();
        }

        ui.separator();
        ui.heading("Fallback 4° Livello: Prezzo Manuale");
        ui.horizontal(|ui| {
            ui.label("Simbolo:");
            ui.add(
                egui::TextEdit::singleline(&mut app.settings_form.manual_symbol_input)
                    .desired_width(90.0),
            );
            ui.label("Prezzo nativo:");
            ui.add(
                egui::TextEdit::singleline(&mut app.settings_form.manual_price_input)
                    .desired_width(80.0),
            );
            if ui.button("Applica prezzo").clicked() {
                if let (Ok(price), Ok(repo)) = (
                    Decimal::from_str(app.settings_form.manual_price_input.trim()),
                    PortfolioRepository::open_file(&app.db_path),
                ) {
                    let currency = if app
                        .settings_form
                        .manual_symbol_input
                        .trim()
                        .eq_ignore_ascii_case(&app.config.market.etf_symbol)
                    {
                        Currency::Eur
                    } else {
                        Currency::Usd
                    };
                    let http = reqwest::Client::new();
                    let bce = BceFxClient::new(http);
                    let chain = FallbackPriceChain::new(Vec::new());
                    let svc = MarketDataService::new(&repo, &chain, &bce);
                    if svc
                        .record_manual_price(
                            app.settings_form.manual_symbol_input.trim(),
                            price,
                            currency,
                        )
                        .is_ok()
                    {
                        let _ = app.reload_from_db();
                        app.status_banner = "Prezzo manuale registrato.".to_string();
                    }
                }
            }
        });
    });
}

fn render_config_editor_col(ui: &mut egui::Ui, app: &mut PacSentinelGuiApp) {
    ui.group(|ui| {
        ui.heading("Configurazione Regole e Soglie (config.toml)");
        egui::ScrollArea::vertical().max_height(310.0).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut app.settings_form.config_toml_draft)
                    .code_editor()
                    .desired_rows(18)
                    .desired_width(f32::INFINITY),
            );
        });

        if ui.button("Valida e Salva Configurazione").clicked() {
            match pac_core::config::PortfolioConfig::from_toml_str(
                &app.settings_form.config_toml_draft,
            ) {
                Ok(new_cfg) => {
                    if std::fs::write(&app.config_path, &app.settings_form.config_toml_draft)
                        .is_ok()
                    {
                        app.config = new_cfg;
                        let _ = app.reload_from_db();
                        app.status_banner = "Configurazione validata e salvata.".to_string();
                    }
                }
                Err(err) => {
                    app.status_banner = format!("Errore validazione config: {err}");
                }
            }
        }
    });
}