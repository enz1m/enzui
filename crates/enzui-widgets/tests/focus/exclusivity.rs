//! Pointer focus must be exclusive: clicking control B has to take
//! focus away from control A.

use iced::widget::column;
use iced::{Element, Point};
use iced_test::Simulator;

/// Click at an absolute point, without needing an id on the target.
fn click_at<M>(ui: &mut Simulator<'_, M>, x: f32, y: f32) {
    ui.point_at(Point::new(x, y));
    let _ = ui.simulate(iced_test::simulator::click());
}

use enzui_widgets::slider::enz_slider;
use iced::Rectangle;

#[derive(Debug, Clone, PartialEq)]
enum W2 {
    A,
    B,
}

/// Intrinsic `enz_slider` height with no label: `hit_height() =
/// thumb_height(20).max(track_height(8)) = 20`.
const SLIDER_H: f32 = 20.0;

/// The layout bounds of the first focused focusable, or `None` if none is.
fn first_focused_bounds<M>(ui: &mut Simulator<'_, M>) -> Option<Rectangle> {
    ui.point_at(Point::new(5000.0, 5000.0));
    ui.find(iced_test::selector::is_focused())
        .ok()
        .map(|t| t.bounds())
}

/// Assert exactly the widget whose click-point is `focused_pt` owns the focus,
/// and the widget at `other_pt` does not.
fn assert_only<M>(ui: &mut Simulator<'_, M>, focused_pt: (f32, f32), other_pt: (f32, f32)) {
    let b = first_focused_bounds(ui).expect("a control must be focused after a click");
    let f = Point::new(focused_pt.0, focused_pt.1);
    let o = Point::new(other_pt.0, other_pt.1);
    assert!(
        b.contains(f),
        "the focused control's bounds {b:?} must contain the clicked point {f:?}"
    );
    assert!(
        !b.contains(o),
        "the focused control's bounds {b:?} must NOT contain the other control's point {o:?} \
         (a stale earlier focus would be returned first and fail here)"
    );
}

/// Slider exclusivity: click slider A, then slider B; only B may be focused.
#[test]
fn slider_focus_is_exclusive() {
    let view = column![
        enz_slider(0.0..=1.0, 0.5, |_| W2::A),
        enz_slider(0.0..=1.0, 0.5, |_| W2::B),
    ];
    let mut ui = Simulator::new(Element::from(view));

    let a = (40.0, SLIDER_H / 2.0);
    let b = (40.0, SLIDER_H + SLIDER_H / 2.0);

    click_at(&mut ui, a.0, a.1);
    assert_only(&mut ui, a, b);

    click_at(&mut ui, b.0, b.1);
    assert_only(&mut ui, b, a);
}
