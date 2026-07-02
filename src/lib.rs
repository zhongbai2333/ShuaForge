pub mod app;
pub mod core;
pub mod desktop;
pub mod mobile;

pub mod ai;
pub mod ai_import;
pub mod deck;
pub mod lan_sync;
pub mod logging;
pub mod problem;
pub mod problem_export;
pub mod self_update;
pub mod store;
pub mod userscript_server;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(android_app: android_activity::AndroidApp) {
    mobile::android::run(android_app);
}
