//! Texts shown by the native parts of the app (tray, notifications), in
//! Italian and English.
//!
//! The webview has its own translations in `src/i18n.ts`: the two MUST stay
//! consistent.

use serde::{Deserialize, Serialize};

/// Language chosen in the preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePref {
    /// Language of the system, if supported, otherwise English.
    #[default]
    System,
    /// Italian.
    It,
    /// English.
    En,
}

/// Language actually in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Italian.
    It,
    /// English.
    En,
}

impl LanguagePref {
    /// Returns the language to use, reading the system locale for `System`.
    pub fn resolve(self) -> Lang {
        match self {
            Self::It => Lang::It,
            Self::En => Lang::En,
            Self::System => match sys_locale::get_locale() {
                Some(locale) if locale.to_lowercase().starts_with("it") => Lang::It,
                _ => Lang::En,
            },
        }
    }
}

/// Native texts in one language; field names say where each one is shown.
pub struct Texts {
    pub service_down: &'static str,
    pub connected: &'static str,
    pub connecting: &'static str,
    pub disconnecting: &'static str,
    pub failed: &'static str,
    pub reconnecting: &'static str,
    pub disconnected: &'static str,
    /// Joins a state and a tunnel name: "Connesso a Ufficio".
    pub to: &'static str,
    /// Decimal separator of the traffic counters.
    pub decimal: &'static str,
    pub internet_blocked: &'static str,
    pub no_tunnels: &'static str,
    pub disconnect: &'static str,
    pub open: &'static str,
    pub quit: &'static str,
    pub notify_connected: &'static str,
    pub notify_reconnected: &'static str,
    pub notify_dropped: &'static str,
    pub notify_dropped_blocked: &'static str,
    pub notify_dropped_open: &'static str,
    pub notify_blocked: &'static str,
    pub notify_blocked_body: &'static str,
    pub notify_protection: &'static str,
}

/// Italian texts.
const IT: Texts = Texts {
    service_down: "Servizio non raggiungibile",
    connected: "Connesso",
    connecting: "Connessione in corso",
    disconnecting: "Disconnessione in corso",
    failed: "Connessione interrotta",
    reconnecting: "Riconnessione in corso",
    disconnected: "Non connesso",
    to: "a",
    decimal: ",",
    internet_blocked: "internet bloccato",
    no_tunnels: "Nessun tunnel importato",
    disconnect: "Disconnetti",
    open: "Apri Submarine",
    quit: "Esci",
    notify_connected: "Connesso",
    notify_reconnected: "Riconnesso",
    notify_dropped: "La VPN si è interrotta",
    notify_dropped_blocked: "Mi riconnetto, intanto il kill switch blocca internet.",
    notify_dropped_open: "Mi riconnetto, intanto il traffico esce senza VPN.",
    notify_blocked: "Internet bloccato",
    notify_blocked_body: "Il kill switch impedisce di uscire senza VPN.",
    notify_protection: "Protezione non attiva",
};

/// English texts.
const EN: Texts = Texts {
    service_down: "Service not reachable",
    connected: "Connected",
    connecting: "Connecting",
    disconnecting: "Disconnecting",
    failed: "Connection lost",
    reconnecting: "Reconnecting",
    disconnected: "Not connected",
    to: "to",
    decimal: ".",
    internet_blocked: "internet blocked",
    no_tunnels: "No tunnels imported",
    disconnect: "Disconnect",
    open: "Open Submarine",
    quit: "Quit",
    notify_connected: "Connected",
    notify_reconnected: "Reconnected",
    notify_dropped: "The VPN dropped",
    notify_dropped_blocked: "Reconnecting, meanwhile the kill switch blocks the internet.",
    notify_dropped_open: "Reconnecting, meanwhile traffic leaves without the VPN.",
    notify_blocked: "Internet blocked",
    notify_blocked_body: "The kill switch keeps traffic from leaving without the VPN.",
    notify_protection: "Protection not active",
};

/// Returns the texts in `lang`.
pub fn texts(lang: Lang) -> &'static Texts {
    match lang {
        Lang::It => &IT,
        Lang::En => &EN,
    }
}
