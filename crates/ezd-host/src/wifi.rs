//! Wi-Fi in the command line: the pairing screen (PIN, fingerprint, QR code) and the
//! `paired` subcommands.

use crate::i18n::fl;
use anyhow::{Result, anyhow};
use clap::{Command, Subcommand, ValueEnum};
use ezd_host_service::store::{ForgetError, PairedDevice, PairedStore, default_config_dir};
use ezd_host_service::{CongestionChoice, PairingCloseReason};
use i18n_embed::fluent::FluentLanguageLoader;
use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CongestionArg {
    Cubic,
    Bbr,
}

impl From<CongestionArg> for CongestionChoice {
    fn from(c: CongestionArg) -> Self {
        match c {
            CongestionArg::Cubic => CongestionChoice::Cubic,
            CongestionArg::Bbr => CongestionChoice::Bbr,
        }
    }
}

// Help texts live in i18n/<lang>/ezd-host.ftl (`paired-*` ids); see `localize`.
#[derive(Subcommand, Debug)]
pub enum PairedCommand {
    /// Lists the paired tablets.
    List {
        #[arg(long, hide = true, value_name = "DIR")]
        config_dir: Option<PathBuf>,
    },
    /// Forgets a paired tablet.
    Forget {
        target: String,
        #[arg(long, hide = true, value_name = "DIR")]
        config_dir: Option<PathBuf>,
    },
}

/// Applies the localized help of the `paired` subcommands.
pub fn localize(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl;
    cmd.mut_subcommand("list", |s| s.about(fl!(l, "paired-list")))
        .mut_subcommand("forget", |s| {
            s.about(fl!(l, "paired-forget"))
                .mut_arg("target", |a| a.help(fl!(l, "paired-forget-target")))
        })
}

fn store(dir: Option<PathBuf>) -> Result<PairedStore> {
    let dir = dir
        .or_else(default_config_dir)
        .ok_or_else(|| anyhow!(fl!("err-no-config-dir")))?;
    Ok(PairedStore::new(&dir))
}

pub fn run(cmd: PairedCommand) -> Result<()> {
    match cmd {
        PairedCommand::List { config_dir } => {
            let store = store(config_dir)?;
            let devices = store.list().map_err(|e| anyhow!(e.to_string()))?;
            print!("{}", render_list(&devices));
            Ok(())
        }
        PairedCommand::Forget { target, config_dir } => {
            let store = store(config_dir)?;
            match store.forget(&target) {
                Ok(d) => {
                    println!(
                        "{}",
                        fl!(
                            "paired-forgot",
                            name = d.name.as_str(),
                            fingerprint = short(&d.fingerprint)
                        )
                    );
                    Ok(())
                }
                Err(ForgetError::NotFound) => Err(anyhow!(fl!(
                    "err-paired-not-found",
                    query = target.as_str()
                ))),
                Err(ForgetError::Ambiguous(n)) => Err(anyhow!(fl!(
                    "err-paired-ambiguous",
                    count = n,
                    query = target.as_str()
                ))),
                Err(ForgetError::Store(e)) => Err(anyhow!(e.to_string())),
            }
        }
    }
}

/// The short dashed form of a hex fingerprint (first 16 digits).
fn short(hex: &str) -> String {
    hex.as_bytes()
        .chunks(4)
        .take(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// The table of `paired list`.
fn render_list(devices: &[PairedDevice]) -> String {
    if devices.is_empty() {
        return format!("{}\n", fl!("paired-none"));
    }
    let never = fl!("paired-never");
    let mut rows = vec![[
        fl!("paired-col-name"),
        fl!("paired-col-fingerprint"),
        fl!("paired-col-paired"),
        fl!("paired-col-last-seen"),
    ]];
    for d in devices {
        rows.push([
            d.name.clone(),
            short(&d.fingerprint),
            format_utc(d.paired_at),
            d.last_seen.map_or_else(|| never.clone(), format_utc),
        ]);
    }
    let widths: Vec<usize> = (0..4)
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for r in &rows {
        let cells: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// `YYYY-MM-DD HH:MM` in UTC for Unix seconds.
pub fn format_utc(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// The QR code of `text` as Unicode half blocks, light modules drawn as blocks (right for a dark
/// terminal) with the quiet zone the scanners need. Error correction is low to keep it small.
pub fn render_qr(text: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(text, EcLevel::L).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

/// What the terminal shows when a pairing window opens.
pub fn print_pairing_open(pin: &str, uri: &str, fingerprint: &str, expires: Instant) {
    println!();
    println!("{}", fl!("pair-open"));
    match render_qr(uri) {
        Some(qr) => println!("{qr}"),
        None => tracing::warn!("cannot render the pairing QR code"),
    }
    println!("{}", fl!("pair-pin", pin = pin));
    println!("{}", fl!("pair-fingerprint", fingerprint = fingerprint));
    println!(
        "{}",
        fl!(
            "pair-expires",
            seconds = expires.saturating_duration_since(Instant::now()).as_secs()
        )
    );
}

pub fn print_paired(device: &str) {
    println!("{}", fl!("pair-done", device = device));
}

pub fn print_pairing_closed(reason: PairingCloseReason) {
    println!(
        "{}",
        match reason {
            PairingCloseReason::Closed => fl!("pair-closed"),
            PairingCloseReason::Paired => fl!("pair-closed-paired"),
            PairingCloseReason::TooManyAttempts => fl!("pair-closed-attempts"),
            PairingCloseReason::Expired => fl!("pair-closed-expired"),
        }
    );
}

pub fn print_listening(addr: std::net::SocketAddr, fingerprint: &str) {
    println!(
        "{}",
        fl!(
            "wifi-listening",
            addr = addr.to_string(),
            fingerprint = fingerprint
        )
    );
    #[cfg(windows)]
    eprintln!("{}", fl!("wifi-firewall-hint", port = addr.port()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01 00:00");
        assert_eq!(format_utc(1_709_210_096), "2024-02-29 12:34");
        assert_eq!(format_utc(1_767_225_540), "2025-12-31 23:59");
    }

    #[test]
    fn short_fingerprint_matches_the_wire_form() {
        assert_eq!(short(&"3f2a91c077de0b14".repeat(4)), "3f2a-91c0-77de-0b14");
    }

    #[test]
    fn qr_is_compact_and_scannable() {
        let uri = format!(
            "ezd://pair?a=192.168.100.200:27183&fp={}&pin=123456&n=Desktop+PC",
            "ab".repeat(32)
        );
        let qr = render_qr(&uri).unwrap();
        let lines: Vec<&str> = qr.lines().collect();
        // Two modules per text line, a quiet zone around: well under a terminal screen.
        assert!((20..=40).contains(&lines.len()), "{}", lines.len());
        assert!(
            lines
                .iter()
                .all(|l| l.chars().count() == lines[0].chars().count())
        );
        assert!(qr.contains('█') && qr.contains(' '));
    }

    #[test]
    fn list_and_forget_use_the_store() {
        let dir = std::env::temp_dir().join(format!("ezd-host-paired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = PairedStore::new(&dir);
        assert_eq!(
            render_list(&store.list().unwrap()).trim(),
            fl!("paired-none")
        );
        store
            .add(&ezd_link::pair::PairedClient {
                fingerprint: [0xab; 32],
                name: "Tab S9".into(),
            })
            .unwrap();
        let table = render_list(&store.list().unwrap());
        assert!(
            table.contains("Tab S9") && table.contains("abab-abab-abab-abab"),
            "{table}"
        );
        run(PairedCommand::Forget {
            target: "tab s9".into(),
            config_dir: Some(dir.clone()),
        })
        .unwrap();
        assert!(store.list().unwrap().is_empty());
        let err = run(PairedCommand::Forget {
            target: "nobody".into(),
            config_dir: Some(dir.clone()),
        })
        .unwrap_err();
        assert!(err.to_string().contains("nobody"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
