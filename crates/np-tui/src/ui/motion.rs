//! 启动页动效曲线：时间相位由调用方给，模块内不持全局时钟。
//!
//! 只提供纯函数：上下浮动 / 左右摇摆（`-1/0/1` 格）、0..1 的呼吸量、
//! 以及只对 RGB 生效的颜色插值。渲染位置、是否允许动、何时重绘都由
//! `splash.rs` 决定。

use ratatui::style::Color;

/// LOGO 上下浮动的正弦周期（秒）。
pub(crate) const BOB_PERIOD_SECONDS: f32 = 3.6;

/// LOGO 左右摇摆的正弦周期（秒）。
pub(crate) const SWAY_PERIOD_SECONDS: f32 = 5.2;

/// 颜色呼吸的余弦周期（秒）。
pub(crate) const BREATH_PERIOD_SECONDS: f32 = 4.2;

/// 把相位换算成弧度；非法输入按 0 处理，避免 NaN 传进布局计算。
fn angle(phase: f32, period: f32) -> f32 {
    if !phase.is_finite() || !period.is_finite() || period <= 0.0 {
        return 0.0;
    }
    std::f32::consts::TAU * phase / period
}

/// 上下浮动：周期 3.6s，四舍五入到 `-1/0/1` 行。
pub(crate) fn bob(phase: f32) -> i16 {
    angle(phase, BOB_PERIOD_SECONDS).sin().round() as i16
}

/// 左右摇摆：周期 5.2s，四舍五入到 `-1/0/1` 列。
pub(crate) fn sway(phase: f32) -> i16 {
    angle(phase, SWAY_PERIOD_SECONDS).sin().round() as i16
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

    fn quarter(period: f32) -> f32 {
        period / 4.0
    }

    #[test]
    fn bob_walks_through_one_cell_and_returns_to_zero() {
        assert_eq!(bob(0.0), 0);
        assert_eq!(bob(quarter(BOB_PERIOD_SECONDS)), 1);
        assert_eq!(bob(BOB_PERIOD_SECONDS / 2.0), 0);
        assert_eq!(bob(BOB_PERIOD_SECONDS * 0.75), -1);
        assert_eq!(bob(BOB_PERIOD_SECONDS), 0);
        assert_eq!(bob(BOB_PERIOD_SECONDS * 10.0), 0);
    }

    #[test]
    fn sway_uses_its_own_period() {
        assert_eq!(sway(0.0), 0);
        assert_eq!(sway(quarter(SWAY_PERIOD_SECONDS)), 1);
        assert_eq!(sway(SWAY_PERIOD_SECONDS / 2.0), 0);
        assert_eq!(sway(SWAY_PERIOD_SECONDS * 0.75), -1);
        assert_eq!(sway(SWAY_PERIOD_SECONDS), 0);
    }

    #[test]
    fn displacement_stays_within_one_cell_for_any_phase() {
        let mut phase = 0.0;
        while phase < BOB_PERIOD_SECONDS * 3.0 {
            assert!((-1..=1).contains(&bob(phase)), "bob {phase}");
            assert!((-1..=1).contains(&sway(phase)), "sway {phase}");
            phase += 0.05;
        }
        assert_eq!(bob(f32::NAN), 0);
        assert_eq!(sway(f32::INFINITY), 0);
    }

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
