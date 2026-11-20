//! Facts about the machine, shared by the reports and the app's support
//! page.

/// Name of the operating system: `PRETTY_NAME` of `/etc/os-release`
/// (`Fedora Linux 45`), on macOS the product and version (`macOS 15.1`),
/// else the platform (`windows`).
pub fn os_name() -> String {
    if cfg!(target_os = "macos") {
        // a program, not a file: asked once (reports may be written while
        // the program is crashing)
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        return NAME
            .get_or_init(|| {
                run("/usr/bin/sw_vers", &["-productVersion"])
                    .and_then(|v| macos_name(&v))
                    .unwrap_or_else(|| std::env::consts::OS.to_owned())
            })
            .clone();
    }
    if let Ok(text) = std::fs::read_to_string("/etc/os-release")
        && let Some(line) = text.lines().find(|l| l.starts_with("PRETTY_NAME="))
    {
        return line["PRETTY_NAME=".len()..].trim_matches('"').to_owned();
    }
    std::env::consts::OS.to_owned()
}

/// Total and available memory in GiB: `/proc/meminfo` on Linux, `sysctl
/// hw.memsize` and `vm_stat` on macOS; `None` elsewhere.
pub fn memory_gib() -> Option<(f64, f64)> {
    if cfg!(target_os = "macos") {
        let bytes = run("/usr/sbin/sysctl", &["-n", "hw.memsize"])?;
        return macos_memory_gib(&bytes, &run("/usr/bin/vm_stat", &[])?);
    }
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<f64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024.0 / 1024.0)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// The standard output of `program` when it succeeds.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `macOS 15.1` for the output of `sw_vers -productVersion`.
fn macos_name(product_version: &str) -> Option<String> {
    let v = product_version.trim();
    let valid = !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.');
    valid.then(|| format!("macOS {v}"))
}

/// (total, available) GiB from `sysctl -n hw.memsize` (bytes) and the output
/// of `vm_stat`. Available is what the system can hand out without paging:
/// free, inactive and speculative pages.
fn macos_memory_gib(memsize: &str, vm_stat: &str) -> Option<(f64, f64)> {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let total: u64 = memsize.trim().parse().ok()?;
    // "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
    let header = vm_stat.lines().next()?;
    let page: u64 = header
        .split("page size of")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |name: &str| -> Option<u64> {
        let line = vm_stat.lines().find(|l| l.starts_with(name))?;
        line.split(':')
            .nth(1)?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    let available = (pages("Pages free")? + pages("Pages inactive")? + pages("Pages speculative")?)
        .checked_mul(page)?;
    let total = total as f64 / GIB;
    Some((total, (available as f64 / GIB).min(total)))
}

#[cfg(test)]
mod tests {
    const VM_STAT: &str = "Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               10000.
Pages active:                            400000.
Pages inactive:                           50000.
Pages speculative:                         4000.
Pages throttled:                              0.
";

    #[test]
    fn macos_name_and_memory_are_read_from_the_system_tools() {
        assert_eq!(super::macos_name("15.1\n").as_deref(), Some("macOS 15.1"));
        assert_eq!(super::macos_name(""), None);
        assert_eq!(super::macos_name("not a version"), None);
        // 16 GiB, (10000 + 50000 + 4000) pages of 16 KiB = 1000 MiB available
        let (total, free) = super::macos_memory_gib("17179869184\n", VM_STAT).unwrap();
        assert!((total - 16.0).abs() < 1e-9, "{total}");
        assert!((free - 1000.0 / 1024.0).abs() < 1e-9, "{free}");
        assert_eq!(super::macos_memory_gib("lots", VM_STAT), None);
        assert_eq!(super::macos_memory_gib("17179869184", "nothing"), None);
        assert_eq!(
            super::macos_memory_gib("17179869184", "(page size of 4096 bytes)\nPages free: 1.\n"),
            None
        );
    }

    #[test]
    fn the_machine_has_a_name() {
        assert!(!super::os_name().is_empty());
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let (total, free) = super::memory_gib().unwrap();
            assert!(total > 0.0 && free <= total);
        }
    }
}
