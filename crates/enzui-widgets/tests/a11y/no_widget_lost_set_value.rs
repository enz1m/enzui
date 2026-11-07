//! Published SetValue capabilities for empty fields and read-only withdrawal.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::secure_field::enz_secure_field;
use iced::advanced::widget::Operation;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
enum Msg {
    #[allow(
        dead_code,
        reason = "the test checks which message arrived, never its payload"
    )]
    Text(String),
    #[allow(
        dead_code,
        reason = "the widget needs these messages to exist; this test never sends them"
    )]
    Action(iced::widget::text_editor::Action),
    Toggle,
}

fn make_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn collect_role(view: Element<'static, Msg, iced::Theme, Renderer>, role: Role) -> Semantics {
    let mut renderer = make_renderer();
    let mut ui = UserInterface::build(
        view,
        Size::new(600.0, 400.0),
        Cache::default(),
        &mut renderer,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    let nodes = collector.into_collected();
    let matching: Vec<_> = nodes.iter().filter(|n| n.semantics.role == role).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one {role:?} node; collected roles were {:?}",
        nodes.iter().map(|n| n.semantics.role).collect::<Vec<_>>()
    );
    matching[0].semantics.clone()
}
#[test]
fn a_secure_field_advertises_set_value_and_does_not_leak() {
    const SECRET: &str = "hunter2-correct-horse";
    let sem = collect_role(
        enz_secure_field(SECRET, Msg::Text, false, Msg::Toggle)
            .label("Password")
            .into(),
        Role::PasswordInput,
    );
    let node = sem.to_accesskit_node();

    assert!(
        node.supports_action(accesskit::Action::SetValue),
        "EnzSecureField is fully editable and exports no value; without a \
         declared capability it would advertise nothing at all"
    );
    assert_eq!(node.value(), None, "the plaintext must never be exported");
    let rendered = format!("{node:?}");
    assert!(
        !rendered.contains(SECRET),
        "the secret appears in the published node: {rendered}"
    );
}
