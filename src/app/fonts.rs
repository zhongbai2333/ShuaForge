use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub fn configure_fonts(ctx: &egui::Context, mobile_touch_height: f32) {
    let mut fonts = FontDefinitions::default();

    register_font(
        &mut fonts,
        "MiSans",
        FontData::from_static(include_bytes!("../../assets/fonts/MiSans-Regular.otf")).tweak(
            egui::FontTweak {
                scale: 1.03,
                y_offset_factor: 0.0,
                y_offset: 0.0,
                baseline_offset_factor: 0.0,
            },
        ),
        true,
    );

    register_font(
        &mut fonts,
        "Noto Sans SC",
        FontData::from_static(include_bytes!("../../assets/fonts/NotoSansSC-VF.ttf")),
        false,
    );

    for (index, (name, data)) in load_system_fonts().into_iter().enumerate() {
        register_font(&mut fonts, &name, data, index == 0);
    }

    ctx.set_fonts(fonts);

    #[cfg(target_os = "android")]
    let is_mobile_font = true;
    #[cfg(not(target_os = "android"))]
    let is_mobile_font = false;

    let mut style = (*ctx.style()).clone();
    let (heading_sz, body_sz, button_sz, small_sz, mono_sz) = if is_mobile_font {
        (24.0, 18.0, 17.0, 15.0, 16.0)
    } else {
        (26.0, 16.0, 15.0, 13.0, 14.0)
    };
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::proportional(heading_sz),
        ),
        (egui::TextStyle::Body, egui::FontId::proportional(body_sz)),
        (
            egui::TextStyle::Button,
            egui::FontId::proportional(button_sz),
        ),
        (egui::TextStyle::Small, egui::FontId::proportional(small_sz)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(mono_sz)),
    ]
    .into();
    if is_mobile_font {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.interact_size = egui::vec2(44.0, mobile_touch_height);
    }
    ctx.set_style(style);
}

fn register_font(fonts: &mut FontDefinitions, name: &str, data: FontData, prefer_first: bool) {
    let name = name.to_owned();
    fonts.font_data.insert(name.clone(), data.into());

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let entries = fonts.families.entry(family).or_default();
        if prefer_first {
            entries.insert(0, name.clone());
        } else {
            entries.push(name.clone());
        }
    }
}

fn load_system_fonts() -> Vec<(String, FontData)> {
    system_font_candidates()
        .iter()
        .filter_map(|(name, path)| {
            std::fs::read(path)
                .ok()
                .map(|bytes| ((*name).to_owned(), FontData::from_owned(bytes)))
        })
        .collect()
}

fn system_font_candidates() -> &'static [(&'static str, &'static str)] {
    &[
        ("Segoe UI", r"C:\Windows\Fonts\segoeui.ttf"),
        ("Segoe UI Symbol", r"C:\Windows\Fonts\seguisym.ttf"),
        ("Segoe UI Emoji", r"C:\Windows\Fonts\seguiemj.ttf"),
        ("Microsoft YaHei", r"C:\Windows\Fonts\msyh.ttc"),
        ("Microsoft YaHei UI", r"C:\Windows\Fonts\msyh.ttf"),
        ("Arial Unicode MS", r"C:\Windows\Fonts\arialuni.ttf"),
        ("SimHei", r"C:\Windows\Fonts\simhei.ttf"),
        ("SimSun", r"C:\Windows\Fonts\simsun.ttc"),
        ("Noto Sans CJK", r"C:\Windows\Fonts\NotoSansCJK-Regular.ttc"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_ui_font_candidates_precede_cjk_fallbacks() {
        let candidates = system_font_candidates();
        let segoe = candidates
            .iter()
            .position(|(name, _)| *name == "Segoe UI")
            .expect("Segoe UI candidate");
        let yahei = candidates
            .iter()
            .position(|(name, _)| *name == "Microsoft YaHei")
            .expect("Microsoft YaHei candidate");

        assert!(segoe < yahei);
    }

    #[test]
    fn bundled_fonts_are_registered_before_system_fallbacks() {
        let mut fonts = FontDefinitions::default();
        register_font(
            &mut fonts,
            "MiSans",
            FontData::from_static(include_bytes!("../../assets/fonts/MiSans-Regular.otf")),
            true,
        );
        register_font(
            &mut fonts,
            "Noto Sans SC",
            FontData::from_static(include_bytes!("../../assets/fonts/NotoSansSC-VF.ttf")),
            false,
        );

        let proportional = fonts
            .families
            .get(&FontFamily::Proportional)
            .expect("proportional family");
        assert_eq!(proportional.first().map(String::as_str), Some("MiSans"));
        assert!(proportional.iter().any(|name| name == "Noto Sans SC"));
    }
}
