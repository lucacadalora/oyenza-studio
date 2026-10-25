fn main() {
    println!("cargo:rerun-if-changed=res/poni.rc");
    println!("cargo:rerun-if-changed=res/poni.manifest");
    println!("cargo:rerun-if-changed=res/poni.ico");
    // Only Windows targets get the icon, manifest (Per-Monitor-V2 DPI) and version info.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Panics with a clear message if rc.exe / x86_64-w64-mingw32-windres is missing.
        embed_resource::compile("res/poni.rc", embed_resource::NONE);
        // MSVC builds: resolve statically imported non-KnownDLLs (d2d1, dwrite,
        // dwmapi, winmm) from System32 only, never from the exe's folder.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        }
    }
    build_info();
}

/// Which build this is, for the menu's About line (src/version.rs): the `ci`
/// run of a release tag builds the release, any other CI run a test build of
/// its commit, and anything else is a local build (no commit or day, so a
/// local rebuild never shows a stale one).
fn build_info() {
    for var in ["GITHUB_ACTIONS", "GITHUB_REF_TYPE", "GITHUB_REF_NAME", "GITHUB_SHA"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let ci = env("GITHUB_ACTIONS") == "true";
    let release = ci && env("GITHUB_REF_TYPE") == "tag" && env("GITHUB_REF_NAME") == format!("v{}", env("CARGO_PKG_VERSION"));
    let kind = if release { "release" } else if ci { "test" } else { "local" };
    let commit: String = if ci { env("GITHUB_SHA").chars().take(7).collect() } else { String::new() };
    let day = if ci { utc_day() } else { String::new() };
    println!("cargo:rustc-env=PONI_BUILD_KIND={kind}");
    println!("cargo:rustc-env=PONI_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=PONI_BUILD_DAY={day}");
}

/// Today in UTC as "25 Oct 2026" (Howard Hinnant's days-to-civil).
fn utc_day() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}
