//! 启动页颜色动效曲线：时间相位由调用方给，模块内不持全局时钟。
//!
//! 只提供纯函数：0..1 的呼吸量，以及只对 RGB 生效的颜色插值。
//! LOGO 位置完全固定，本模块不参与任何布局计算。

use ratatui::style::Color;

/// 颜色呼吸的余弦周期（秒）。
pub(crate) const BREATH_PERIOD_SECONDS: f32 = 4.2;

/// 把相位换算成弧度；非法输入按 0 处理，避免 NaN 传进颜色计算。
fn angle(phase: f32, period: f32) -> f32 {
    if !phase.is_finite() || !period.is_finite() || period <= 0.0 {
        return 0.0;
    }
    std::f32::consts::TAU * phase / period
}

/// 颜色呼吸量：0 起步，半个周期到 1，再回到 0；始终在 `0..=1`。
pub(crate) fn breath(phase: f32) -> f32 {
    let value = (1.0 - angle(phase, BREATH_PERIOD_SECONDS).cos()) / 2.0;
    value.clamp(0.0, 1.0)
}

/// 只对 `Color::Rgb` 插值；任一端不是 RGB（Reset / ANSI / 索引色）时返回 `a`。
///
/// `t` 会夹到 `0..=1`，非有限值按 0 处理。
pub(crate) fn blend(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Color::Rgb(lerp(ar, br, t), lerp(ag, bg, t), lerp(ab, bb, t))
}

fn lerp(from: u8, to: u8, t: f32) -> u8 {
    let value = f32::from(from) + (f32::from(to) - f32::from(from)) * t;
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breath_starts_at_zero_and_stays_in_unit_interval() {
        assert_eq!(breath(0.0), 0.0);
        assert!((breath(BREATH_PERIOD_SECONDS / 2.0) - 1.0).abs() < 0.001);
        assert!(breath(BREATH_PERIOD_SECONDS).abs() < 0.001);
        let mut phase = 0.0;
        while phase < BREATH_PERIOD_SECONDS * 2.0 {
            assert!((0.0..=1.0).contains(&breath(phase)), "breath {phase}");
            phase += 0.02;
        }
    }

    #[test]
    fn blend_interpolates_rgb_only() {
        assert_eq!(
            blend(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255), 0.5),
            Color::Rgb(128, 128, 128)
        );
        assert_eq!(
            blend(Color::Rgb(10, 20, 30), Color::Rgb(200, 100, 50), 0.0),
            Color::Rgb(10, 20, 30)
        );
        assert_eq!(
            blend(Color::Rgb(10, 20, 30), Color::Rgb(200, 100, 50), 1.0),
            Color::Rgb(200, 100, 50)
        );
        // 非 RGB 一端不做暗改：返回 a 本身。
        assert_eq!(blend(Color::Reset, Color::Rgb(1, 2, 3), 0.5), Color::Reset);
        assert_eq!(
            blend(Color::Rgb(1, 2, 3), Color::Indexed(9), 0.5),
            Color::Rgb(1, 2, 3)
        );
        // t 越界 / 非有限值不越出 RGB 范围。
        assert_eq!(
            blend(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255), 5.0),
            Color::Rgb(255, 255, 255)
        );
        assert_eq!(
            blend(Color::Rgb(7, 8, 9), Color::Rgb(255, 255, 255), f32::NAN),
            Color::Rgb(7, 8, 9)
        );
    }
}
