//! Mobile platform boundary.
//!
//! `mobile` is intentionally shared by Android and iOS. Keep platform SDK
//! usage inside `mobile::android` or `mobile::ios` so future shared mobile UI
//! and state can live here without depending on one platform.

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "ios")]
pub mod ios;
