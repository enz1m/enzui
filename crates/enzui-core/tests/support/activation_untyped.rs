use super::activation_child::ReapedChild;
use std::time::Instant;

pub(super) fn assert_untyped_context_needs_platform(
    context: enzui_core::activation::ActivationContext,
) {
    use enzui_core::activation::{ActivationHandoffError, ChildLaunch, ChildLaunchError};
    match ChildLaunch::new("/bin/true")
        .activation(context)
        .spawn(Instant::now())
    {
        Err(ChildLaunchError::Activation(ActivationHandoffError::UnspecifiedPlatform)) => {}
        Ok(child) => {
            let _child = ReapedChild(child);
            panic!("untyped native context silently selected a child protocol");
        }
        Err(error) => panic!("unexpected native context refusal: {error:?}"),
    }
}
