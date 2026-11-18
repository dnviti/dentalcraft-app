//! Facts about the machine, shared by the reports and the app's support
//! page.

/// Name of the operating system: `PRETTY_NAME` of `/etc/os-release`
/// (`Fedora Linux 45`), else the platform (`windows`, `macos`).
pub fn os_name() -> String {
    if let Ok(text) = std::fs::read_to_string("/etc/os-release")
        && let Some(line) = text.lines().find(|l| l.starts_with("PRETTY_NAME="))
    {
        return line["PRETTY_NAME=".len()..].trim_matches('"').to_owned();
    }
    std::env::consts::OS.to_owned()
}

/// Total and available memory in GiB (`/proc/meminfo`; Linux only).
pub fn memory_gib() -> Option<(f64, f64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<f64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024.0 / 1024.0)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_machine_has_a_name() {
        assert!(!super::os_name().is_empty());
        if cfg!(target_os = "linux") {
            let (total, free) = super::memory_gib().unwrap();
            assert!(total > 0.0 && free <= total);
        }
    }
}
