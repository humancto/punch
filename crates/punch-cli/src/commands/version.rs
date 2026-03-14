//! `punch version` — Print version and build information.

/// Supported LLM providers.
pub const SUPPORTED_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "groq",
    "ollama",
    "deepseek",
    "lmstudio",
];

/// Format the version info block.
pub fn format_version_info() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let homepage = env!("CARGO_PKG_HOMEPAGE");

    let mut lines = Vec::new();
    lines.push(format!("punch {}", version));
    lines.push(format!("Homepage: {}", homepage));
    lines.push(String::new());

    // Build info.
    lines.push("Build Info:".to_string());
    lines.push(format!(
        "  Rust:     {}",
        option_env!("RUSTC_VERSION").unwrap_or("unknown")
    ));
    lines.push(format!(
        "  Target:   {}",
        option_env!("TARGET").unwrap_or(std::env::consts::ARCH)
    ));
    lines.push(format!("  OS:       {}", std::env::consts::OS));
    lines.push(format!(
        "  Profile:  {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    ));
    lines.push(String::new());

    // Supported providers.
    lines.push("Supported Providers:".to_string());
    for provider in SUPPORTED_PROVIDERS {
        lines.push(format!("  - {}", provider));
    }

    lines.join("\n")
}

/// Run the version command.
pub fn run() -> i32 {
    println!("{}", format_version_info());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_version_info_contains_version() {
        let info = format_version_info();
        assert!(info.contains("punch"));
        assert!(info.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn test_format_version_info_contains_build_info() {
        let info = format_version_info();
        assert!(info.contains("Build Info:"));
        assert!(info.contains("Rust:"));
        assert!(info.contains("OS:"));
        assert!(info.contains("Profile:"));
    }

    #[test]
    fn test_format_version_info_contains_providers() {
        let info = format_version_info();
        assert!(info.contains("Supported Providers:"));
        assert!(info.contains("anthropic"));
        assert!(info.contains("openai"));
        assert!(info.contains("ollama"));
    }

    #[test]
    fn test_supported_providers_list() {
        assert!(SUPPORTED_PROVIDERS.contains(&"anthropic"));
        assert!(SUPPORTED_PROVIDERS.contains(&"openai"));
        assert!(SUPPORTED_PROVIDERS.contains(&"google"));
        assert!(SUPPORTED_PROVIDERS.contains(&"groq"));
        assert!(SUPPORTED_PROVIDERS.contains(&"ollama"));
        assert!(SUPPORTED_PROVIDERS.contains(&"deepseek"));
        assert!(SUPPORTED_PROVIDERS.contains(&"lmstudio"));
    }

    #[test]
    fn test_run_returns_zero() {
        // run() prints and returns 0.
        assert_eq!(run(), 0);
    }

    #[test]
    fn test_debug_profile_detected() {
        let info = format_version_info();
        // In test builds, we're in debug mode.
        assert!(info.contains("debug"));
    }

    #[test]
    fn test_os_field_present() {
        let info = format_version_info();
        let os = std::env::consts::OS;
        assert!(info.contains(os));
    }

    #[test]
    fn test_homepage_present() {
        let info = format_version_info();
        assert!(info.contains("Homepage:"));
    }
}
