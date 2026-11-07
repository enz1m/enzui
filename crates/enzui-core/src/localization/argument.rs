use super::Value;

/// A message argument that `localized!` borrows without consuming application state.
pub trait MessageArgument {
    /// Supplies display text or a numeric value that retains Fluent plural behavior.
    fn message_value(&self) -> Value<'_>;
}

impl MessageArgument for str {
    fn message_value(&self) -> Value<'_> {
        Value::from(self)
    }
}

impl MessageArgument for String {
    fn message_value(&self) -> Value<'_> {
        self.as_str().message_value()
    }
}

impl<T: MessageArgument + ?Sized> MessageArgument for &T {
    fn message_value(&self) -> Value<'_> {
        (*self).message_value()
    }
}

impl<T: MessageArgument + ?Sized> MessageArgument for std::sync::Arc<T> {
    fn message_value(&self) -> Value<'_> {
        self.as_ref().message_value()
    }
}

impl MessageArgument for std::borrow::Cow<'_, str> {
    fn message_value(&self) -> Value<'_> {
        self.as_ref().message_value()
    }
}

impl MessageArgument for Value<'_> {
    fn message_value(&self) -> Value<'_> {
        self.clone()
    }
}

impl MessageArgument for bool {
    fn message_value(&self) -> Value<'_> {
        Value::from(if *self { "true" } else { "false" })
    }
}

macro_rules! numeric_argument {
    ($($number:ty),+ $(,)?) => {
        $(impl MessageArgument for $number {
            fn message_value(&self) -> Value<'_> { Value::from(self) }
        })+
    };
}

numeric_argument!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
);
