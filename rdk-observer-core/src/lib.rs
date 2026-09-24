//! Core observation types and behavior for RDK Observer.

/// Returns the version of the core crate.
#[must_use]
pub const fn crate_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::crate_version;

    #[test]
    fn crate_version_matches_package_version() {
        assert_eq!(crate_version(), env!("CARGO_PKG_VERSION"));
    }
}
