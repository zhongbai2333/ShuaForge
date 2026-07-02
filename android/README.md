# ShuaForge Android POC

这是 ShuaForge 的 Android Studio 工程壳，目标是先让现有 Rust/egui 应用在 Android 模拟器中启动和调试。

## 前置要求

- Android Studio
- Android SDK Platform 35
- Android NDK 30.0.14904198（或修改 `app/build.gradle` 中的 `ndkVersion`）
- Rust Android targets：
  - `aarch64-linux-android`
  - `x86_64-linux-android`

## 在 Android Studio 中打开

1. 使用 Android Studio 打开本目录：`android/`
2. 等待 Gradle Sync 完成。
3. 选择一个 Android 模拟器（建议 x86_64）。
4. 运行 `app`。

Gradle 的 `cargoBuildDebug` 任务会在 APK 打包前自动执行。为了让 Android Studio 模拟器调试更快，Debug 默认只构建 `x86_64`：

- 调用 `cargo build --lib --target x86_64-linux-android`
- 将生成的 `libshuaforge_core.so` 复制到 `app/build/rustJniLibs/`

如果需要同时构建真机和模拟器 ABI，可以传入 Gradle 属性：

```powershell
.\gradlew.bat :app:assembleDebug -PshuaforgeAbis=arm64-v8a,x86_64
```

## 当前范围

Android POC 当前不包含桌面油猴采集桥；移动端定位为：

- 题库文件导入（后续接入 Android 系统文件选择器）
- AI 导入（后续接入文本粘贴/文件选择）
- 本地刷题
- AI 配置和解析

桌面端仍负责油猴采集和整理题库。
