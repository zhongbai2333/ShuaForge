# ShuaForge

ShuaForge 是一个用 Rust + egui 开发的轻量桌面刷题助手。目标是像背单词一样刷题：导入题库后逐题作答，答对就从本轮题库移除，答错就重新加入队列，并显示本地或 AI 错题解析。

本项目采用 `MIT License`，更适合桌面端和 iOS App Store 这类分发场景。

## 当前 MVP 功能

- 导入 JSON / CSV 题库
- 随机出题
- 答对：题目从本轮队列移除
- 答错：题目重新加入队尾
- 错题解析：优先显示题库自带解析，可选调用 AI 接口
- AI 配置：支持自定义 endpoint、model、API key、超时时间
- 轻量桌面 UI：使用 `eframe/egui`，不嵌浏览器，不走 Electron
- 本地 SQLite 题库：导入后自动保存为题库卡片，重复导入同一路径会合并更新
- 题库卡片管理：可从单个题库开始，也可从全部题库开始
- 题组文件夹：可创建题组，把多个题库拖进去组合成一个大题组一起练习
- 答题历史
- 题型控件：单选题用 Radio，多选题用 Checkbox，文本题用 TextBox
- 顺序 / 乱序刷题切换

## 为什么走 egui？

- 纯 Rust，跨平台友好
- UI 代码和业务逻辑都在同一个 Rust 工程里，迭代快
- 相比 WebView/Electron 路线更轻
- 适合先做桌面端 MVP，后续可再拆核心逻辑复用到 CLI / 移动端 / Web

## 快速运行

确保已安装 Rust 工具链，然后在项目根目录运行：

```powershell
cargo run
```

## 发布构建产物

Release 工作流会在推送 `v*` 标签或手动触发时构建并上传这些桌面端产物：

- Windows x64：`.zip`，包含 `shuaforge.exe` 与 README
- Linux x64：`.zip`，包含 `shuaforge` 与 README
- Linux arm64：`.zip`，包含 `shuaforge` 与 README
- macOS x64：`.dmg`
- macOS arm64：`.dmg`

注意：GitHub 已下架 macOS Intel hosted runner，因此 macOS x64 产物会在 `macos-latest` 上通过 `x86_64-apple-darwin` target 交叉编译；macOS arm64 产物使用 `aarch64-apple-darwin` target 构建。

CI、PR 标签构建和 Release 发布共用 `.github/workflows/desktop-build.yml` 中的同一套桌面端构建矩阵，避免某个平台的打包逻辑只在发布时才暴露问题。

打开后点击“导入题库”，可选择：

- `examples/problems.sample.json`
- `examples/problems.sample.csv`

导入后题库会保存到本机 SQLite 数据库。Windows 默认位置类似：

```text
%LOCALAPPDATA%\ShuaForge\shuaforge.sqlite3
```

再次启动程序时会先进入题库主页，不会自动进入答题。主页类似阅读器书架：

- 点击单个题库卡片：练习该题库
- 点击 `全部题库开始`：从所有题目中练习
- 创建题组文件夹：把多个题库组合成一个大题组
- 拖动题库卡片到题组文件夹：把题库加入题组
- 点击题组文件夹：练习该题组下所有题库的合集

## 题库格式

### JSON

```json
[
	{
		"id": "rust-001",
		"prompt": "Rust 中用于表示所有权借用的符号是什么？",
		"answer": "&",
		"explanation": "&T 表示不可变借用，&mut T 表示可变借用。",
		"tags": ["rust", "ownership"]
	}
]
```

### CSV

CSV 需要包含这些表头：

```csv
id,prompt,answer,explanation,tags
rust-002,Rust 中 Result<T E> 通常用于表达什么？,可恢复错误,Result 用于返回成功值 Ok 或错误值 Err,"rust,error-handling"
```

CSV 的 `tags` 支持逗号分隔，导入后会转换成标签列表。

题型会自动推断：

- 选项为 `A. ...` / `B. ...` 且答案为单个字母：单选题
- 选项为 `A. ...` / `B. ...` 且答案包含多个字母：多选题
- 没有选择项：文本题

JSON 也可以显式提供 `problem_type`：

```json
{
	"id": "example-001",
	"prompt": "下列哪些属于推断统计？\nA. 参数估计\nB. 绘图\nC. 假设检验",
	"answer": "A,C",
	"explanation": "推断统计包括参数估计和假设检验。",
	"tags": ["统计学"],
	"problem_type": "multiple_choice"
}
```

## 题库导出油猴脚本

项目内置一个只读导出脚本：`userscripts/shuaforge-question-exporter.user.js`。

该文件是可直接安装到 Tampermonkey / Violentmonkey 的编译产物；维护源码位于：`userscripts/src/shuaforge-question-exporter.user.ts`。

它参考 OCS 这类网课脚本的“多平台页面适配”思路，但功能边界不同：**只从已完成答题/考试结果/答案解析页面提取题目数据，不自动答题、不提交表单、不修改页面数据**。

### 安装方式

1. 浏览器安装 Tampermonkey / Violentmonkey / 脚本猫等脚本管理器。
2. 新建脚本。
3. 将 `userscripts/shuaforge-question-exporter.user.js` 的内容复制进去保存。
4. 打开已完成答题结果页。

### 开发 / 编译

如果修改导出器逻辑，请改 TypeScript 源码：

```powershell
userscripts/src/shuaforge-question-exporter.user.ts
```

首次开发先安装依赖：

```powershell
npm install
```

类型检查：

```powershell
npm run check:userscript
```

编译生成可安装脚本：

```powershell
npm run build:userscript
```

编译后会更新：

```powershell
userscripts/shuaforge-question-exporter.user.js
```

### 使用方式

1. 页面右下角会出现 `ShuaForge 题库导出` 面板。
2. 推荐先点击 `选择区域`，框住题目列表的大容器。
3. 点击 `扫描预览`，确认识别到的题目数量和答案。
4. 点击 `导出 CSV`。
5. 在 ShuaForge 桌面端点击 `导入题库`，选择导出的 CSV。

### 导出字段

- `id`：根据题目顺序和题干哈希生成
- `prompt`：题干与选项
- `answer`：正确答案 / 参考答案 / 标准答案
- `explanation`：答案解析
- `tags`：知识点 / 考点 / 标签
- `deck_name`：题库 / 章节 / 作业名称，例如“第一章 数据统计导论”
- `deck_info`：题量、满分、得分、作答时间等页面元信息

### 当前识别规则

脚本会从页面文本中识别这些特征：

- `正确答案` / `参考答案` / `标准答案`
- `我的答案` / `你的答案`
- `答案解析` / `解析`
- `知识点` / `考点` / `标签`
- `A.` / `B.` / `C.` 这类选项

如果平台 DOM 很特殊，可以先用 `选择区域` 精确框住题目列表，提高识别率。

## AI 配置

可从界面载入 `examples/ai-config.sample.json`，或在界面里填写后保存。

```json
{
	"enabled": false,
	"endpoint": "https://api.example.com/v1/chat/completions",
	"api_key": "",
	"model": "your-model-name",
	"timeout_secs": 30
}
```

AI 响应当前兼容两类格式：

- `{ "explanation": "..." }`
- OpenAI-like：`{ "choices": [{ "message": { "content": "..." } }] }`

如果不启用 AI，答错时会显示题库自带 `explanation` 或本地提示。

## 后续路线

- 题库编辑器
- 按标签筛选刷题
- 错题本与复习间隔算法（SM-2 / Anki 风格）
- 本地进度持久化
- 更丰富的判题模式：关键词、正则、多答案、代码题单元测试
- AI 生成相似题 / 总结薄弱点
