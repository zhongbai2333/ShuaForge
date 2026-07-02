use crate::{app::ShuaForgeApp, store};
use android_activity::AndroidApp;
use jni::{
    EnvUnowned, JavaVM, jni_sig, jni_str,
    objects::{JClass, JObject, JString, Reference},
    refs::Global,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

static CURRENT_ANDROID_APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
static FILE_PICKER_RESULTS: OnceLock<Mutex<VecDeque<Result<PathBuf, String>>>> = OnceLock::new();

fn current_android_app() -> &'static Mutex<Option<AndroidApp>> {
    CURRENT_ANDROID_APP.get_or_init(|| Mutex::new(None))
}

fn file_picker_results() -> &'static Mutex<VecDeque<Result<PathBuf, String>>> {
    FILE_PICKER_RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub fn run(android_app: android_activity::AndroidApp) {
    log::info!("ShuaForge Android entry starting");

    if let Ok(mut current) = current_android_app().lock() {
        *current = Some(android_app.clone());
    }

    if let Some(data_dir) = android_app.internal_data_path() {
        log::info!("ShuaForge Android data dir: {}", data_dir.display());
        store::set_android_data_dir(data_dir);
    } else {
        log::warn!("Android internal data path is unavailable; falling back to platform data dir");
    }

    let options = eframe::NativeOptions {
        android_app: Some(android_app),
        ..Default::default()
    };

    if let Err(err) = eframe::run_native(
        "ShuaForge 刷题助手",
        options,
        Box::new(|cc| Ok(Box::new(ShuaForgeApp::new_mobile(cc)))),
    ) {
        log::error!("ShuaForge Android run_native failed: {err}");
    }

    if let Ok(mut current) = current_android_app().lock() {
        *current = None;
    }
}

pub fn request_problem_bank_file_picker() -> Result<(), String> {
    let app = current_android_app()
        .lock()
        .ok()
        .and_then(|current| current.clone())
        .ok_or_else(|| "Android Activity 尚未就绪，无法打开系统文件选择器。".to_owned())?;

    app.clone().run_on_java_main_thread(Box::new(move || {
        let jvm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as _) };
        let result = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let activity: jni::sys::jobject = app.activity_as_ptr() as _;
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity)? };
            env.call_method(
                activity.as_ref(),
                jni_str!("openProblemBankFilePicker"),
                jni_sig!(() -> ()),
                &[],
            )?;
            Ok(())
        });
        if let Err(err) = result {
            log::error!("Failed to request Android file picker: {err:?}");
            push_file_picker_result(Err(format!("打开系统文件选择器失败：{err:?}")));
        }
    }));

    Ok(())
}

pub fn poll_problem_bank_file_picker_result() -> Option<Result<PathBuf, String>> {
    file_picker_results()
        .lock()
        .ok()
        .and_then(|mut results| results.pop_front())
}

fn push_file_picker_result(result: Result<PathBuf, String>) {
    if let Ok(mut results) = file_picker_results().lock() {
        results.push_back(result);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_zhongbai_shuaforge_ShuaForgeActivity_nativeOnProblemBankFilePickerResult(
    mut env: EnvUnowned<'_>,
    _class: JClass<'_>,
    path: JString<'_>,
    error: JString<'_>,
) {
    let outcome = env.with_env(|_env| -> Result<(), jni::errors::Error> {
        let path = jni_string_to_rust(&path);
        let error = jni_string_to_rust(&error);

        if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
            push_file_picker_result(Err(error));
        } else if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
            push_file_picker_result(Ok(PathBuf::from(path)));
        } else {
            push_file_picker_result(Err("系统文件选择器没有返回文件。".into()));
        }
        Ok(())
    });
    outcome.resolve::<jni::errors::LogErrorAndDefault>();
}

fn jni_string_to_rust(value: &JString<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    Some(value.to_string())
}
