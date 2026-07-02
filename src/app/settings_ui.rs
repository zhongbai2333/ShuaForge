use super::*;

impl ShuaForgeApp {
    pub(super) fn render_settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("练习设置");
        let mut shuffled = self.practice_order == PracticeOrder::Shuffled;
        if ui.checkbox(&mut shuffled, "乱序练习").changed() {
            self.practice_order = if shuffled {
                PracticeOrder::Shuffled
            } else {
                PracticeOrder::Sequential
            };
            self.status = if shuffled {
                "已切换为乱序练习。"
            } else {
                "已切换为顺序练习。"
            }
            .into();
            self.persist_settings();
        }

        ui.separator();
        ui.heading("AI 解析");
        let mut settings_changed = false;
        settings_changed |= ui
            .checkbox(&mut self.ai_config.enabled, "启用 AI 错题解析")
            .changed();
        ui.horizontal_wrapped(|ui| {
            if ui.button("导入 JSON").clicked() {
                self.load_ai_config();
            }
            if ui.button("导出 JSON").clicked() {
                self.save_ai_config();
            }
        });
        ui.small("常用：开关与配置备份。连接参数通常只在首次配置时修改。");
        ui.collapsing("连接参数与高级选项", |ui| {
            ui.label("Endpoint");
            settings_changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.ai_config.endpoint)
                        .desired_width(ui.available_width()),
                )
                .changed();
            ui.label("Model");
            settings_changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.ai_config.model)
                        .desired_width(ui.available_width()),
                )
                .changed();
            ui.label("Fast Model（知识点/预生成解析）");
            settings_changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.ai_config.fast_model)
                        .desired_width(ui.available_width()),
                )
                .changed();
            ui.label("API Key（仅保存在本机 SQLite 设置中）");
            settings_changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.ai_config.api_key)
                        .password(true)
                        .desired_width(ui.available_width()),
                )
                .changed();
            settings_changed |= ui
                .add(
                    egui::Slider::new(&mut self.ai_config.knowledge_point_concurrency, 1..=256)
                        .text("AI 并发"),
                )
                .changed();
            settings_changed |= ui
                .add(egui::Slider::new(&mut self.ai_config.timeout_secs, 5..=120).text("超时秒数"))
                .changed();
            ui.small("并发过高遇到 429 时再调低；JSON 导入/导出可用于备份或迁移配置。");
        });
        if settings_changed {
            self.persist_settings();
        }

        ui.separator();
        ui.heading("公式渲染");
        let mut formula_changed = false;
        ui.small(if self.formula_render_settings.enable_remote {
            "当前：本地即时显示，在线服务在后台增强复杂公式。"
        } else {
            "当前：仅使用本地渲染，最快且不联网。"
        });
        formula_changed |= ui
            .checkbox(
                &mut self.formula_render_settings.enable_remote,
                "使用在线服务增强复杂 LaTeX 公式",
            )
            .changed();
        if self.formula_render_settings.enable_remote {
            egui::ComboBox::from_label("在线服务")
                .selected_text(formula_render_short_label(formula_render_preset_label(
                    &self.formula_render_settings.remote_url_template,
                )))
                .width(ui.available_width().min(220.0))
                .show_ui(ui, |ui| {
                    for (label, url) in crate::app::markdown::FORMULA_RENDER_PRESETS {
                        if ui
                            .selectable_label(
                                self.formula_render_settings.remote_url_template == *url,
                                formula_render_short_label(label),
                            )
                            .clicked()
                        {
                            self.formula_render_settings.remote_url_template = (*url).to_owned();
                            formula_changed = true;
                        }
                    }
                    let _ = ui.selectable_label(
                        !is_formula_render_preset(
                            &self.formula_render_settings.remote_url_template,
                        ),
                        "自定义服务",
                    );
                });
        }
        ui.collapsing("高级：自定义在线公式服务", |ui| {
            ui.label("URL 模板（{latex} 会替换为 URL 编码后的公式）");
            formula_changed |= ui
                .add(
                    egui::TextEdit::singleline(
                        &mut self.formula_render_settings.remote_url_template,
                    )
                    .desired_width(ui.available_width()),
                )
                .changed();
            formula_changed |= ui
                .add(
                    egui::Slider::new(
                        &mut self.formula_render_settings.remote_timeout_secs,
                        1..=20,
                    )
                    .text("超时秒数"),
                )
                .changed();
            if ui.button("恢复默认 CodeCogs").clicked()
                && let Some((_, url)) = crate::app::markdown::FORMULA_RENDER_PRESETS.first()
            {
                self.formula_render_settings.remote_url_template = (*url).to_owned();
                formula_changed = true;
            }
            ui.small("在线服务默认异步加载：不会卡住答题；失败会保留本地渲染。公共免费服务不保证可用，公式内容会发送到所选服务。需要稳定性可自建兼容服务。")
        });
        if formula_changed {
            self.persist_settings();
            self.status = if self.formula_render_settings.enable_remote {
                "已更新公式渲染设置：远程服务启用，失败会自动回退本地渲染。"
            } else {
                "已关闭远程公式渲染，使用本地渲染。"
            }
            .into();
        }

        ui.separator();
        ui.heading("局域网同步");
        ui.small("同一局域网内，把另一台设备的题库同步到当前设备。仅需要时启动。");
        ui.horizontal_wrapped(|ui| {
            if ui.button("启动同步服务").clicked() {
                self.start_lan_sync_server();
            }
            if ui.button("刷新设备列表").clicked() {
                lan_sync::ensure_discovery_listener();
                self.sync_status = "已刷新设备列表；请确保另一台设备也启动了同步服务。".into();
            }
        });
        if let Some(addr) = &self.sync_server_addr {
            ui.small(format!("本机同步地址：{addr}"));
        }
        ui.small(&self.sync_status);
        let peers = lan_sync::discovered_peers();
        if peers.is_empty() {
            ui.small("暂未发现设备。两台设备需连接同一局域网，并至少一台已启动同步服务。");
        } else {
            for peer in peers {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{} · {}", peer.device_name, peer.addr));
                    let syncing = self.sync_receiver.is_some();
                    if ui
                        .add_enabled(!syncing, egui::Button::new("从此设备同步"))
                        .clicked()
                    {
                        self.start_lan_sync_import(peer.addr.clone(), peer.device_name.clone());
                    }
                });
            }
        }
        if self.sync_receiver.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在同步题库...");
            });
        }

        ui.separator();
        ui.heading("状态");
        ui.label(egui::RichText::new(&self.status).small());
        ui.small(format!(
            "{} 道题 · {} 个题库 · {} 个题组",
            self.bank_count,
            self.deck_cards.len(),
            self.group_cards.len()
        ));
        ui.collapsing("状态详情", |ui| {
            if let Some(name) = &self.active_deck_name {
                ui.label(format!("当前题库：{name}"));
            }
            if let Some(path) = &self.loaded_path {
                ui.small(format!("题库：{}", path.display()));
            }
            if let Some(path) = &self.ai_config_path {
                ui.small(format!("AI 配置：{}", path.display()));
            }
        });

        ui.separator();
        ui.heading("浏览器采集脚本");
        #[cfg(target_os = "android")]
        {
            ui.small(
                "Android 端不内置油猴采集桥。请从桌面端同步题库，或导入题库文件/使用 AI 导入。",
            );
        }
        #[cfg(not(target_os = "android"))]
        {
            ui.small("安装一次脚本，打开题库页会自动加入采集队列。常用操作在题库主页的“从浏览器获取题库”。");
            ui.horizontal_wrapped(|ui| {
                if ui.button("检查/启动本地采集服务").clicked() {
                    match crate::userscript_server::ensure_bridge_running() {
                        Ok(url) => {
                            self.status = format!(
                                "本地采集服务已就绪：{url}。已安装脚本的题库页面会自动连接；回到题库页后在 ShuaForge 点击“从浏览器获取题库”即可采集。"
                            );
                        }
                        Err(err) => {
                            self.status = err;
                        }
                    }
                }
                if ui.button("安装/更新浏览器脚本").clicked() {
                    match crate::userscript_server::open_userscript_install_page() {
                        Ok(url) => {
                            self.status = format!(
                                "已打开浏览器脚本安装页：{url}。首次使用、脚本更新或服务端口变更时安装一次即可；之后打开题库页会自动连接 ShuaForge。"
                            );
                        }
                        Err(err) => {
                            self.status = err;
                        }
                    }
                }
            });
            ui.collapsing("使用说明", |ui| {
                ui.small("1. 首次使用或脚本更新时，点击“安装/更新浏览器脚本”。");
                ui.small("2. 打开题库/解析页面，页面会自动连接 ShuaForge。");
                ui.small("3. 回到 ShuaForge 题库主页，点击“从浏览器获取题库”。");
                ui.small("本地采集服务会随程序自动启动；可同时打开多个题库页。")
            });
        }

        ui.separator();
        ui.collapsing("答题历史", |ui| {
            for record in &self.answer_history {
                ui.small(format!(
                    "{} {} {}：{} / {}",
                    record.answered_at,
                    if record.is_correct { "✅" } else { "❌" },
                    record.problem_id,
                    record.user_answer,
                    record.correct_answer
                ));
            }
        });
    }
}

fn formula_render_preset_label(url_template: &str) -> &'static str {
    crate::app::markdown::FORMULA_RENDER_PRESETS
        .iter()
        .find(|(_, url)| *url == url_template)
        .map(|(label, _)| *label)
        .unwrap_or("自定义服务")
}

fn formula_render_short_label(label: &'static str) -> &'static str {
    label
        .split('（')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(label)
}

fn is_formula_render_preset(url_template: &str) -> bool {
    crate::app::markdown::FORMULA_RENDER_PRESETS
        .iter()
        .any(|(_, url)| *url == url_template)
}
