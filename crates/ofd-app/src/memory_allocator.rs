//! Avoid multiplying large raster allocation high-water marks across glibc
//! thread arenas. This is process-local tuning, not an RSS limit or a trim loop.

pub(crate) fn configure() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if should_limit_arenas(
        std::env::var_os("MALLOC_ARENA_MAX").is_some(),
        std::env::var("GLIBC_TUNABLES").ok().as_deref(),
    ) {
        unsafe extern "C" {
            fn mallopt(parameter: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // glibc malloc.h: M_ARENA_MAX = -8. Must run before font/GPUI workers
        // start. Respect explicit allocator tuning supplied by the launcher.
        unsafe {
            mallopt(-8, 2);
        }
    }
}

#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
fn should_limit_arenas(explicit_max: bool, tunables: Option<&str>) -> bool {
    !explicit_max
        && !tunables.is_some_and(|value| {
            value
                .split(':')
                .any(|entry| entry.starts_with("glibc.malloc.arena_max="))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_allocator_overrides_are_respected() {
        assert!(should_limit_arenas(false, None));
        assert!(should_limit_arenas(
            false,
            Some("glibc.malloc.trim_threshold=1")
        ));
        assert!(!should_limit_arenas(true, None));
        assert!(!should_limit_arenas(
            false,
            Some("glibc.malloc.arena_max=4")
        ));
        assert!(!should_limit_arenas(
            false,
            Some("x=1:glibc.malloc.arena_max=2:y=1")
        ));
    }
}
