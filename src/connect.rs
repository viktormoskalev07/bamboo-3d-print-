//! Отправка через официальное приложение Bambu Connect (вход по email, печать через облако).
//! Программа открывает готовый .gcode.3mf в Bambu Connect по ссылке
//! bambu-connect://import-file?path=...&name=...&version=1.0.0

use anyhow::{bail, Result};
use std::path::Path;

pub const DOWNLOAD_URL: &str = "https://bambulab.com/en/software/bambu-connect";

pub fn is_installed() -> bool {
    #[cfg(windows)]
    {
        // быстрая проверка по файлу (стандартные места установки)
        let mut candidates = Vec::new();
        if let Ok(l) = std::env::var("LOCALAPPDATA") {
            candidates.push(Path::new(&l).join(r"Programs\bambu-connect\Bambu Connect.exe"));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(p) = std::env::var(pf) {
                candidates.push(Path::new(&p).join(r"Bambu Connect\Bambu Connect.exe"));
                candidates.push(Path::new(&p).join(r"bambu-connect\Bambu Connect.exe"));
            }
        }
        if candidates.iter().any(|p| p.exists()) {
            return true;
        }
        // иначе — зарегистрирован ли протокол bambu-connect://
        use std::os::windows::process::CommandExt;
        std::process::Command::new("reg")
            .args(["query", r"HKCR\bambu-connect"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW

            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

pub fn open(file: &Path, display_name: &str) -> Result<()> {
    if !is_installed() {
        bail!("Bambu Connect не установлен. Скачайте: {DOWNLOAD_URL}\nПосле установки войдите в нём по email и повторите.");
    }
    let abs = std::path::absolute(file)?;
    let abs = abs.display().to_string();
    let abs = abs.trim_start_matches(r"\\?\");
    let name = display_name;
    let url = format!(
        "bambu-connect://import-file?path={}&name={}&version=1.0.0",
        encode(abs),
        encode(name)
    );
    #[cfg(windows)]
    std::process::Command::new("rundll32.exe").arg("url.dll,FileProtocolHandler").arg(&url).spawn()?;
    #[cfg(not(windows))]
    std::process::Command::new("xdg-open").arg(&url).spawn()?;
    Ok(())
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
