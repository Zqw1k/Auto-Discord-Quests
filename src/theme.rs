use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Pos2, Rect, Stroke, StrokeKind, TextStyle, Vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(9, 11, 16);
pub const SURFACE: Color32 = Color32::from_rgb(17, 20, 28);
pub const CARD: Color32 = Color32::from_rgb(21, 24, 33);
pub const CARD_HOVER: Color32 = Color32::from_rgb(25, 29, 40);
pub const INPUT: Color32 = Color32::from_rgb(13, 16, 23);
pub const BORDER: Color32 = Color32::from_rgb(33, 38, 50);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(27, 31, 42);
pub const TEXT: Color32 = Color32::from_rgb(233, 236, 244);
pub const TEXT_SOFT: Color32 = Color32::from_rgb(154, 162, 180);
pub const MUTED: Color32 = Color32::from_rgb(108, 116, 134);
pub const ACCENT: Color32 = Color32::from_rgb(88, 101, 242);
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(128, 139, 255);
pub const SUCCESS: Color32 = Color32::from_rgb(61, 205, 132);
pub const WARNING: Color32 = Color32::from_rgb(245, 176, 65);
pub const DANGER: Color32 = Color32::from_rgb(240, 86, 96);
pub const VIDEO: Color32 = Color32::from_rgb(78, 190, 255);
pub const GAME: Color32 = Color32::from_rgb(139, 148, 255);
pub const ACTIVITY: Color32 = Color32::from_rgb(184, 140, 255);
pub const STREAM: Color32 = Color32::from_rgb(236, 120, 202);

pub fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn setup(context: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "bvp-regular".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/BeVietnamPro-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "bvp-medium".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/BeVietnamPro-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "bvp-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/BeVietnamPro-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "bvp-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/BeVietnamPro-Bold.ttf"
        ))),
    );

    fonts
        .families
        .insert(FontFamily::Proportional, vec!["bvp-regular".into()]);
    fonts
        .families
        .insert(FontFamily::Name("medium".into()), vec!["bvp-medium".into()]);
    fonts.families.insert(
        FontFamily::Name("semibold".into()),
        vec!["bvp-semibold".into()],
    );
    fonts
        .families
        .insert(FontFamily::Name("bold".into()), vec!["bvp-bold".into()]);
    context.set_fonts(fonts);

    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(9.0, 9.0);
    style.spacing.button_padding = Vec2::new(13.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.scroll.bar_width = 8.0;
    style.text_styles = [
        (TextStyle::Heading, semibold(22.0)),
        (TextStyle::Body, regular(14.0)),
        (TextStyle::Button, medium(13.5)),
        (TextStyle::Small, regular(11.5)),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_corner_radius = CornerRadius::same(16);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.faint_bg_color = Color32::from_rgb(24, 28, 38);
    visuals.extreme_bg_color = INPUT;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = ACCENT_LIGHT;
    visuals.selection.bg_fill = tint(ACCENT, 110);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_LIGHT);
    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.noninteractive.weak_bg_fill = CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_SOFT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 32, 43);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(28, 32, 43);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(10);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(37, 42, 57);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(37, 42, 57);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(62, 70, 92));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(10);
    visuals.widgets.active.bg_fill = tint(ACCENT, 190);
    visuals.widgets.active.weak_bg_fill = tint(ACCENT, 190);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_LIGHT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(10);
    visuals.widgets.open.bg_fill = Color32::from_rgb(32, 36, 49);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(32, 36, 49);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.open.corner_radius = CornerRadius::same(10);

    style.visuals = visuals;
    context.set_style(style);
}

pub fn panel_shadow() -> Shadow {
    Shadow {
        offset: [0, 18],
        blur: 48,
        spread: 0,
        color: Color32::from_black_alpha(110),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Bolt,
    Play,
    Stop,
    Check,
    Refresh,
    Alert,
    Clock,
    Gift,
    Close,
    Logout,
    Search,
    Sparkle,
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let center = rect.center();
    let size = rect.width().min(rect.height());
    let half = size * 0.5;
    let stroke_width = (size * 0.095).clamp(1.2, 2.2);
    let stroke = Stroke::new(stroke_width, color);
    let points = |pairs: &[(f32, f32)]| -> Vec<Pos2> {
        pairs
            .iter()
            .map(|(x, y)| Pos2::new(center.x + x * half, center.y + y * half))
            .collect()
    };

    match icon {
        Icon::Bolt => {
            painter.add(egui::Shape::convex_polygon(
                points(&[
                    (0.12, -0.72),
                    (-0.42, 0.12),
                    (-0.04, 0.12),
                    (-0.14, 0.76),
                    (0.44, -0.14),
                    (0.06, -0.14),
                ]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                points(&[(-0.34, -0.52), (0.52, 0.0), (-0.34, 0.52)]),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, Vec2::splat(size * 0.52)),
                CornerRadius::same(2),
                color,
            );
        }
        Icon::Check => {
            painter.add(egui::Shape::line(
                points(&[(-0.5, 0.02), (-0.12, 0.38), (0.52, -0.34)]),
                stroke,
            ));
        }
        Icon::Refresh => {
            let radius = half * 0.62;
            let mut arc = Vec::new();
            for step in 0..40 {
                let angle = -1.2 + (step as f32 / 39.0) * 5.2;
                arc.push(center + Vec2::new(angle.cos(), angle.sin()) * radius);
            }
            painter.add(egui::Shape::line(arc, stroke));
            painter.add(egui::Shape::line(
                vec![
                    center + Vec2::new(radius * 0.78, -radius * 1.12),
                    center + Vec2::new(radius * 1.24, -radius * 0.58),
                    center + Vec2::new(radius * 0.58, -radius * 0.45),
                ],
                stroke,
            ));
        }
        Icon::Alert => {
            let triangle = points(&[(0.0, -0.62), (0.68, 0.54), (-0.68, 0.54)]);
            painter.add(egui::Shape::closed_line(triangle, stroke));
            painter.circle_filled(
                center + Vec2::new(0.0, half * 0.12),
                stroke_width * 0.9,
                color,
            );
            painter.line_segment(
                [
                    center + Vec2::new(0.0, half * -0.22),
                    center + Vec2::new(0.0, half * 0.02),
                ],
                stroke,
            );
        }
        Icon::Clock => {
            painter.circle_stroke(center, half * 0.66, stroke);
            painter.line_segment([center, center + Vec2::new(0.0, -half * 0.38)], stroke);
            painter.line_segment(
                [center, center + Vec2::new(half * 0.3, half * 0.08)],
                stroke,
            );
        }
        Icon::Gift => {
            let box_rect = Rect::from_center_size(
                center + Vec2::new(0.0, half * 0.14),
                Vec2::new(size * 0.78, size * 0.56),
            );
            painter.rect_stroke(box_rect, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(box_rect.left(), box_rect.top() + box_rect.height() * 0.28),
                    Pos2::new(box_rect.right(), box_rect.top() + box_rect.height() * 0.28),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x, box_rect.top()),
                    Pos2::new(center.x, box_rect.bottom()),
                ],
                stroke,
            );
            painter.add(egui::Shape::line(
                points(&[
                    (0.0, -0.32),
                    (-0.34, -0.52),
                    (-0.28, -0.16),
                    (0.0, -0.24),
                    (0.28, -0.16),
                    (0.34, -0.52),
                    (0.0, -0.32),
                ]),
                stroke,
            ));
        }
        Icon::Close => {
            painter.line_segment(
                [
                    center + Vec2::new(-half * 0.4, -half * 0.4),
                    center + Vec2::new(half * 0.4, half * 0.4),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(half * 0.4, -half * 0.4),
                    center + Vec2::new(-half * 0.4, half * 0.4),
                ],
                stroke,
            );
        }
        Icon::Logout => {
            let door = Rect::from_min_max(
                Pos2::new(center.x - half * 0.55, center.y - half * 0.62),
                Pos2::new(center.x + half * 0.05, center.y + half * 0.62),
            );
            painter.rect_stroke(door, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(center.x - half * 0.1, center.y),
                    Pos2::new(center.x + half * 0.66, center.y),
                ],
                stroke,
            );
            painter.add(egui::Shape::line(
                points(&[(0.38, -0.22), (0.66, 0.0), (0.38, 0.22)]),
                stroke,
            ));
        }
        Icon::Search => {
            painter.circle_stroke(
                center + Vec2::new(-half * 0.12, -half * 0.12),
                half * 0.42,
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(half * 0.18, half * 0.18),
                    center + Vec2::new(half * 0.6, half * 0.6),
                ],
                stroke,
            );
        }
        Icon::Sparkle => {
            let sparkle = points(&[
                (0.0, -0.7),
                (0.16, -0.16),
                (0.7, 0.0),
                (0.16, 0.16),
                (0.0, 0.7),
                (-0.16, 0.16),
                (-0.7, 0.0),
                (-0.16, -0.16),
            ]);
            painter.add(egui::Shape::convex_polygon(
                sparkle.clone(),
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::closed_line(
                sparkle,
                Stroke::new(1.0_f32, color),
            ));
        }
    }
}

/// Soft radial falloff built as a triangle fan so there are no visible rings.
pub fn glow(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    let segments = 64;
    for step in 0..=segments {
        let angle = step as f32 / segments as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + Vec2::angled(angle) * radius, Color32::TRANSPARENT);
    }
    for step in 0..segments {
        mesh.add_triangle(0, step as u32 + 1, step as u32 + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 64x64 window icon: blurple rounded square with a white bolt.
pub fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    let bolt = [
        (0.55, 0.10),
        (0.29, 0.53),
        (0.47, 0.53),
        (0.43, 0.90),
        (0.72, 0.44),
        (0.53, 0.44),
    ];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = (x as f32 + 0.5) / SIZE as f32;
            let fy = (y as f32 + 0.5) / SIZE as f32;
            let radius = 0.24f32;
            let qx = (fx - 0.5).abs() - (0.5 - radius);
            let qy = (fy - 0.5).abs() - (0.5 - radius);
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;

            let coverage = (0.5 - distance * SIZE as f32).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }

            let t = (fx + fy) * 0.5;
            let base = Color32::from_rgb(
                lerp_u8(88, 150, t),
                lerp_u8(101, 96, t),
                lerp_u8(242, 255, t),
            );

            let inside_bolt = point_in_polygon(fx, fy, &bolt);
            let color = if inside_bolt { Color32::WHITE } else { base };

            let index = (y * SIZE + x) * 4;
            rgba[index] = color.r();
            rgba[index + 1] = color.g();
            rgba[index + 2] = color.b();
            rgba[index + 3] = (coverage * 255.0) as u8;
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

fn lerp_u8(from: u8, to: u8, t: f32) -> u8 {
    (from as f32 + (to as f32 - from as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn point_in_polygon(x: f32, y: f32, polygon: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}
