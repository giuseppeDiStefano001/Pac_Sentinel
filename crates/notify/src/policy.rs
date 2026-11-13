use crate::error::NotifyError;
use chrono::{DateTime, Duration, Timelike};
use chrono_tz::Tz;
use pac_core::config::NotificationPolicyConfig;
use pac_core::types::{AssetId, SignalLevel};
use pac_data::db::PortfolioRepository;

/// Verifica se l'orario corrente `Europe/Rome` ricade nelle ore silenziose configurate.
pub fn is_quiet_hours(now_rome: DateTime<Tz>, cfg: &NotificationPolicyConfig) -> bool {
    let hour = now_rome.hour();
    let start = cfg.quiet_hours_start_hour;
    let end = cfg.quiet_hours_end_hour;

    if start == end {
        return false;
    }
    if start > end {
        // Es. 23:00 -> 07:00 (attraversa la mezzanotte)
        hour >= start || hour < end
    } else {
        hour >= start && hour < end
    }
}

/// Motore anti-spam che decide se una notifica di segnale deve essere inviata
/// in base a: pausa temporanea, ore silenziose, cambio di livello e cooldown.
pub struct NotificationGatekeeper<'a> {
    repo: &'a PortfolioRepository,
    cfg: &'a NotificationPolicyConfig,
}

impl<'a> NotificationGatekeeper<'a> {
    pub fn new(repo: &'a PortfolioRepository, cfg: &'a NotificationPolicyConfig) -> Self {
        Self { repo, cfg }
    }

    pub fn is_muted(&self, now_rome: DateTime<Tz>) -> Result<bool, NotifyError> {
        if let Some(until) = self.repo.get_mute_until()? {
            if now_rome < until {
                return Ok(true);
            }
            self.repo.set_mute_until(None)?;
        }
        Ok(false)
    }

    /// Valuta se inviare la notifica per `asset` al livello `new_level`.
    /// Ritorna `Some(previous_level_opt)` se l'invio è autorizzato, `None` se soppresso dall'anti-spam.
    pub fn should_notify_asset_signal(
        &self,
        asset: AssetId,
        new_level: SignalLevel,
        now_rome: DateTime<Tz>,
    ) -> Result<Option<Option<SignalLevel>>, NotifyError> {
        if self.is_muted(now_rome)? {
            return Ok(None);
        }

        if is_quiet_hours(now_rome, self.cfg)
            && !(new_level == SignalLevel::Rosso && self.cfg.allow_red_during_quiet_hours)
        {
            return Ok(None);
        }

        let key = format!("asset_signal:{}", asset.code());
        let prev_state = self.repo.get_signal_state(&key)?;

        match prev_state {
            None => {
                // Primo avvio: notifica solo se non è VERDE (evita spam al bootstrap)
                self.repo.upsert_signal_state(&key, new_level, now_rome)?;
                if new_level > SignalLevel::Verde {
                    Ok(Some(None))
                } else {
                    Ok(None)
                }
            }
            Some(prev) => {
                if prev.level != new_level {
                    // Cambiamento di stato in qualsiasi direzione
                    self.repo.upsert_signal_state(&key, new_level, now_rome)?;
                    Ok(Some(Some(prev.level)))
                } else if new_level > SignalLevel::Verde {
                    // Stesso livello non-verde: rispetta cooldown
                    let elapsed = now_rome.signed_duration_since(prev.last_notified_at_rome);
                    if elapsed >= Duration::hours(self.cfg.cooldown_hours_same_level) {
                        self.repo.upsert_signal_state(&key, new_level, now_rome)?;
                        Ok(Some(Some(prev.level)))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Verifica un evento puntuale deduplicato (es. promemoria PAC mensile, passaggio Fase 2,
    /// earnings giorno prima, dati stale una sola volta, riepilogo giornaliero).
    pub fn try_acquire_dedup_event(
        &self,
        event_key: &str,
        is_critical: bool,
        now_rome: DateTime<Tz>,
    ) -> Result<bool, NotifyError> {
        if self.is_muted(now_rome)? {
            return Ok(false);
        }
        if is_quiet_hours(now_rome, self.cfg) && !is_critical {
            return Ok(false);
        }
        if self.repo.has_dedup_event(event_key)? {
            return Ok(false);
        }
        self.repo.record_dedup_event(event_key, now_rome)?;
        Ok(true)
    }
}