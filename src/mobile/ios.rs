//! iOS startup boundary.
//!
//! iOS will need a host-provided native app/window integration before this can
//! mirror Android startup. Keep this module separate so Android-only APIs never
//! leak into shared mobile code.

pub fn platform_name() -> &'static str {
    "ios"
}
